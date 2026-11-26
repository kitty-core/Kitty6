//! Windows of title memory with no card side leave out the 4 KiB pages that
//! hold what they last copied (`VICTORY_WINDOW_SKIP_SAME`, [`mode`]; off by
//! default).
//!
//! A window outside buffers whose one buffer the CPU fills (in the PC's
//! memory, or in a GPU upload heap with `VICTORY_WINDOW_UPLOAD=1`) has
//! nothing beside it to compare with, so every page the write watch or the
//! guards name is copied into it whole. A page written need not hold other
//! bytes: the title rewrites constant data. Measured in AnyXboxX: 67 to 73
//! MB a frame of what such windows took were the bytes they had taken last
//! time.
//!
//! With `1` each window keeps a 64-bit print of the bytes it last copied
//! for each 4 KiB page ([`page_print`], 8 bytes a page: 8 KiB for a window of
//! 4 MiB), and a page whose bytes give the same print is not copied again.
//!
//! What it rests on: the window's buffer holds what the window last copied
//! for each page. Nothing but its copies writes it: no shader writes a
//! window (run 91 and since), and a window that shaders are said to write
//! (`Shadow::writing`) compares nothing from then on. A window let go takes
//! its prints with it, so one made again for the same memory starts with
//! none; a page copied only in part has none. A page that cannot be read is
//! read as zeros (`crate::host::copy_mapped`), which is what the copy as
//! before stores in the buffer, so its print is that of the buffer's bytes.
//! shortcut: two pages that differ and give the same print (about once in
//! 2^64) leave the window with the older bytes until the title writes the
//! page again.
//!
//! Ported from AnyXboxX PR #340 (VICTORY_WINDOW_SKIP_SAME, by Skyfrye),
//! where it is for windows with a card side and no CPU side; here for the
//! windows with no card side, whose buffer is the one the CPU fills.

use std::sync::atomic::{AtomicU64, Ordering};

/// What a window with no card side does with a 4 KiB page it takes that
/// holds the bytes it last copied for the page ([`mode`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkipSame {
    /// Unset or `0`, the default: copies it again and compares nothing, the
    /// code path as before the switch.
    Off,
    /// `sample`: copies it again; one page in 16, picked by its address
    /// ([`sampled_page`]), is read again and compared, for the frame line.
    Sample,
    /// `1`: leaves it out. The window's buffer holds those bytes.
    Skip,
}

/// `VICTORY_WINDOW_SKIP_SAME` ([`SkipSame`]; read once). (AnyXboxX samples
/// when unset; here unset is off, so a run without the switch does no work
/// of it at all.)
pub(crate) fn mode() -> SkipSame {
    static MODE: std::sync::OnceLock<SkipSame> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_WINDOW_SKIP_SAME").ok().as_deref().map(str::trim) {
        Some("1") => SkipSame::Skip,
        Some("sample") => SkipSame::Sample,
        _ => SkipSame::Off,
    })
}

/// The pages windows keep a print of.
const PRINTED: usize = 4 << 10;

/// Bytes of the 4 KiB pages windows took since the last frame line, by what
/// they were ([`Sent`]): the bytes the window last copied there, other
/// bytes, nothing to compare with (never copied, a page taken in part, or a
/// window shaders write). With [`SkipSame::Sample`] the pages compared, 16
/// times over: an estimate.
static SAME: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// Bytes of title memory windows took since the last frame line: copied,
/// and left out ([`SkipSame::Skip`]).
static TAKEN: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Bytes of the windows' prints alive.
static PRINT_BYTES: AtomicU64 = AtomicU64::new(0);

/// With the frame line, with the switch set: what windows with no card side
/// took of title memory per frame over `frames`, copied and left out, and
/// against what they last copied ([`SAME`]). Taken and reset.
pub(crate) fn line(frames: f64) {
    let skip = match mode() {
        SkipSame::Off => return,
        SkipSame::Skip => "1: left out",
        SkipSame::Sample => "sample: copied again, one 4 KiB page in 16 compared",
    };
    let mb = |n: &AtomicU64| n.swap(0, Ordering::Relaxed) as f64 / 1e6 / frames;
    let [sent, left] = TAKEN.each_ref().map(mb);
    let [same, other, unknown] = SAME.each_ref().map(mb);
    tracing::info!(
        "title memory windows with no card side took per frame: {sent:.1} MB copied, {left:.1} MB left out; of what they \
         took, {same:.1} MB held the bytes the window last copied for their 4 KiB page, {other:.1} MB other bytes, \
         {unknown:.1} MB nothing to compare with; prints {:.1} MB (VICTORY_WINDOW_SKIP_SAME={skip})",
        PRINT_BYTES.load(Ordering::Relaxed) as f64 / 1e6
    );
}

/// A window made prints for `n` pages.
pub(crate) fn prints_made(n: usize) {
    PRINT_BYTES.fetch_add(n as u64 * 8, Ordering::Relaxed);
}

/// A window with prints for `n` pages is gone.
pub(crate) fn prints_gone(n: usize) {
    PRINT_BYTES.fetch_sub(n as u64 * 8, Ordering::Relaxed);
}

/// A window took `sent` bytes of title memory into its buffer and left out
/// `left`.
pub(crate) fn taken(sent: usize, left: usize) {
    for (count, bytes) in TAKEN.iter().zip([sent, left]) {
        if bytes != 0 {
            count.fetch_add(bytes as u64, Ordering::Relaxed);
        }
    }
}

/// A window shaders are said to write took `bytes` and compared nothing.
pub(crate) fn unknown(bytes: usize) {
    SAME[Sent::Unknown as usize].fetch_add(bytes as u64, Ordering::Relaxed);
}

/// [`SAME`] takes `tally`.
pub(crate) fn tally_add(tally: &[u64; 3]) {
    for (n, &bytes) in SAME.iter().zip(tally) {
        if bytes != 0 {
            n.fetch_add(bytes, Ordering::Relaxed);
        }
    }
}

/// What the bytes of a 4 KiB page a window takes are against its print of
/// what it last copied for the page: the index into [`SAME`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent {
    Same = 0,
    Other = 1,
    Unknown = 2,
}

/// Keys of [`page_print`]: 24 for the words of the 64-byte stripes (stripe
/// `n` of a 1 KiB block takes keys `n` to `n + 7`), then 8 to start the
/// lanes with and to mix them by. SplitMix64's numbers from 0.
const PRINT_KEYS: [u64; 32] = {
    let mut keys = [0u64; 32];
    let (mut x, mut i) = (0u64, 0);
    while i < 32 {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        keys[i] = z ^ (z >> 31);
        i += 1;
    }
    keys
};

/// A 64-bit print of a page's bytes: equal bytes give equal prints, and two
/// different pages the same print about once in 2^64. Eight lanes take the
/// 64-bit words of each 64-byte stripe as XXH3 accumulates them: a word is
/// added to the next lane, and the product of its halves, keyed by its
/// place in the stripe and the stripe's in its 1 KiB block, to its own; the
/// lanes are mixed after each block and folded into one at the end, with
/// the bytes left over. Some 40 GB/s on one core (AnyXboxX's measure). Not
/// a hash anyone can attack: the title does not aim at it.
fn page_print(bytes: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut lanes = [0u64; 8];
    for (i, lane) in lanes.iter_mut().enumerate() {
        *lane = PRINT_KEYS[24 + i] ^ bytes.len() as u64;
    }
    let mut stripes = bytes.chunks_exact(64);
    for (n, stripe) in (&mut stripes).enumerate() {
        let key = n % 16;
        let mut words = [0u64; 8];
        for (word, b) in words.iter_mut().zip(stripe.chunks_exact(8)) {
            *word = u64::from_le_bytes(b.try_into().unwrap_or([0; 8]));
        }
        for i in 0..8 {
            let x = words[i] ^ PRINT_KEYS[key + i];
            lanes[i ^ 1] = lanes[i ^ 1].wrapping_add(words[i]);
            lanes[i] = lanes[i].wrapping_add((x & 0xffff_ffff).wrapping_mul(x >> 32));
        }
        if key == 15 {
            for (i, lane) in lanes.iter_mut().enumerate() {
                *lane = (*lane ^ (*lane >> 47) ^ PRINT_KEYS[24 + i]).wrapping_mul(0x9e37_79b1);
            }
        }
    }
    let mut rest = stripes.remainder().chunks_exact(8);
    let words = rest.by_ref().map(|w| u64::from_le_bytes(w.try_into().unwrap_or([0; 8])));
    let mut h = K;
    for word in lanes.into_iter().chain(words) {
        h = (h ^ word).wrapping_mul(K);
        h ^= h >> 29;
    }
    for &b in rest.remainder() {
        h = (h ^ u64::from(b)).wrapping_mul(K);
    }
    h ^ (h >> 32)
}

/// Of 4 KiB page `index` of a window, `bytes` are taken (`whole`: all of
/// the page that lies in the window): what they are against the page's
/// print, which becomes theirs. A part of a page leaves it none: the buffer
/// then holds bytes of two takings.
fn page_sent(prints: &mut [u64], index: usize, whole: bool, bytes: &[u8]) -> Sent {
    let Some(print) = prints.get_mut(index) else {
        return Sent::Unknown;
    };
    if !whole {
        *print = 0;
        return Sent::Unknown;
    }
    // 0 is no print.
    let now = page_print(bytes).max(1);
    match std::mem::replace(print, now) {
        was if was == now => Sent::Same,
        0 => Sent::Unknown,
        _ => Sent::Other,
    }
}

/// The prints of a window over `[start, end)`: one per 4 KiB page that lies
/// in it.
pub(crate) fn prints_for(start: usize, end: usize) -> usize {
    (end - 1) / PRINTED - start / PRINTED + 1
}

/// [`SkipSame::Skip`]: title memory `[from, to)` of a window over `[start,
/// end)`, read by `read` (address, bytes to fill; what cannot be read is
/// filled with zeros) into `scratch` 256 KiB at a time; the runs of 4 KiB
/// pages that do not hold the bytes the window last copied for them go to
/// `send` (address, bytes), in order, and the others are left out.
/// `prints`: the window's, one per page from `start`'s on ([`prints_for`]).
/// What each page was goes to `tally`, in bytes. Returns the bytes sent.
pub(crate) fn copy(
    (start, end): (usize, usize),
    (from, to): (usize, usize),
    prints: &mut [u64],
    scratch: &mut Vec<u8>,
    tally: &mut [u64; 3],
    mut read: impl FnMut(usize, &mut [u8]),
    mut send: impl FnMut(usize, &[u8]),
) -> usize {
    // Pages never cross a block: what is read stays in the core's cache
    // while it is printed and copied.
    const BLOCK: usize = 256 << 10;
    if scratch.len() < BLOCK {
        scratch.resize(BLOCK, 0);
    }
    let first = start / PRINTED;
    let (mut sent, mut at) = (0, from);
    let mut runs: Vec<(usize, usize)> = Vec::new();
    while at < to {
        let stop = ((at / BLOCK + 1) * BLOCK).min(to);
        let bytes = &mut scratch[..stop - at];
        read(at, bytes);
        runs.clear();
        for page in at / PRINTED..stop.div_ceil(PRINTED) {
            let (a, b) = ((page * PRINTED).max(at), ((page + 1) * PRINTED).min(stop));
            let whole = a == (page * PRINTED).max(start) && b == ((page + 1) * PRINTED).min(end);
            let what = page_sent(prints, page - first, whole, &bytes[a - at..b - at]);
            tally[what as usize] += (b - a) as u64;
            if what == Sent::Same {
                continue;
            }
            match runs.last_mut() {
                Some(last) if last.1 == a => last.1 = b,
                _ => runs.push((a, b)),
            }
        }
        for &(a, b) in &runs {
            send(a, &bytes[a - at..b - at]);
            sent += b - a;
        }
        at = stop;
    }
    sent
}

/// Whether 4 KiB page `page` of title memory (its number) is one of the one
/// in 16 that [`SkipSame::Sample`] compares: picked by a multiply of its
/// number, so the same pages each time, spread over the 64 KiB pages and the
/// windows.
fn sampled_page(page: usize) -> bool {
    (page as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 == 0
}

/// [`SkipSame::Sample`]: of title memory `[from, to)` a window over
/// `[start, end)` has just copied, the 4 KiB pages [`sampled_page`] picks
/// are read by `read` (address, bytes to fill; false: not all there) and
/// compared with the window's `prints`, which take them; what they were goes
/// to `tally`, 16 times their bytes. Nothing is left out.
pub(crate) fn sample(
    (start, end): (usize, usize),
    (from, to): (usize, usize),
    prints: &mut [u64],
    tally: &mut [u64; 3],
    mut read: impl FnMut(usize, &mut [u8]) -> bool,
) {
    let first = start / PRINTED;
    let mut page_bytes = [0u8; PRINTED];
    for page in (from / PRINTED..to.div_ceil(PRINTED)).filter(|&p| sampled_page(p)) {
        let (a, b) = ((page * PRINTED).max(from), ((page + 1) * PRINTED).min(to));
        let whole = a == (page * PRINTED).max(start) && b == ((page + 1) * PRINTED).min(end);
        let bytes = &mut page_bytes[..b - a];
        // A page not all there now is as one taken in part.
        let what = page_sent(prints, page - first, whole && read(a, bytes), bytes);
        tally[what as usize] += 16 * (b - a) as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::{PRINTED, Sent, copy, page_print, page_sent, prints_for, sample, sampled_page};

    /// Title memory for the tests: bytes that differ from page to page and
    /// within each.
    fn memory(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    /// Equal bytes give equal prints; a page with one bit flipped, two
    /// words or two stripes swapped, a word moved by a byte, or one byte
    /// longer gives another.
    #[test]
    fn a_print_changes_with_any_change_of_the_page() {
        let page = memory(PRINTED, 7);
        let print = page_print(&page);
        assert_eq!(page_print(&page.clone()), print);
        for bit in 0..PRINTED * 8 {
            let mut other = page.clone();
            other[bit / 8] ^= 1 << (bit % 8);
            assert_ne!(page_print(&other), print, "bit {bit}");
        }
        let swapped = |a: usize, b: usize, n: usize| {
            let mut other = page.clone();
            for i in 0..n {
                other.swap(a + i, b + i);
            }
            page_print(&other)
        };
        // Words in one stripe, in two stripes of a block, in two blocks;
        // whole stripes the same keys meet in other blocks; the halves of a
        // word.
        let swaps = [(0, 8, 8), (8, 72, 8), (64, 1088, 8), (0, 64, 64), (64, 1088, 64), (0, 1024, 1024), (0, 4, 4)];
        for (a, b, n) in swaps {
            assert_ne!(swapped(a, b, n), print, "{a} {b} {n}");
        }
        // Pages of zeros and of one repeated word, their sign bits flipped
        // two at a time.
        let zeros = vec![0u8; PRINTED];
        let mut two = zeros.clone();
        two[7] ^= 0x80;
        two[15] ^= 0x80;
        assert_ne!(page_print(&two), page_print(&zeros));
        let mut ones = vec![0u8; PRINTED];
        ones.iter_mut().step_by(8).for_each(|b| *b = 1);
        assert_ne!(page_print(&ones), page_print(&zeros));
        let mut shifted = page.clone();
        shifted.rotate_right(1);
        assert_ne!(page_print(&shifted), print);
        assert_ne!(page_print(&page[..PRINTED - 1]), page_print(&page[..PRINTED - 2]));
        assert_ne!(page_print(&page[..13]), page_print(&page[..12]));
    }

    /// A page is the same when its bytes give the print last taken, unknown
    /// the first time and when taken in part (which leaves it no print),
    /// other when its bytes changed.
    #[test]
    fn a_page_is_the_same_only_after_the_same_bytes_were_taken_whole() {
        let mut prints = vec![0u64; 3];
        let (a, b) = (memory(PRINTED, 1), memory(PRINTED, 2));
        assert_eq!(page_sent(&mut prints, 1, true, &a), Sent::Unknown);
        assert_eq!(page_sent(&mut prints, 1, true, &a), Sent::Same);
        assert_eq!(page_sent(&mut prints, 1, true, &b), Sent::Other);
        assert_eq!(page_sent(&mut prints, 1, true, &a), Sent::Other);
        assert_eq!(page_sent(&mut prints, 1, false, &a[..100]), Sent::Unknown);
        assert_eq!(prints[1], 0, "a page taken in part keeps no print");
        assert_eq!(page_sent(&mut prints, 1, true, &a), Sent::Unknown);
        assert_eq!(page_sent(&mut prints, 1, true, &a), Sent::Same);
        assert_eq!((prints[0], prints[2]), (0, 0), "the other pages are not touched");
        // Past the window: nothing to compare with.
        assert_eq!(page_sent(&mut prints, 3, true, &a), Sent::Unknown);
    }

    /// A window over title memory, refreshed again and again over the
    /// ranges asked while the title writes some pages with other bytes and
    /// rewrites others with the bytes they hold: with the pages that hold
    /// what was last copied left out, the window's buffer holds the title's
    /// bytes over every range refreshed, and nothing is copied of a page the
    /// title only rewrote.
    #[test]
    fn the_buffer_holds_the_titles_bytes_with_the_same_pages_left_out() {
        const START: usize = 0x40_0000;
        const LEN: usize = 1 << 20;
        let mut title = memory(LEN, 3);
        let mut buffer = vec![0xeeu8; LEN];
        let mut prints = vec![0u64; prints_for(START, START + LEN)];
        let mut scratch = Vec::new();
        let mut refresh = |title: &[u8], buffer: &mut [u8], prints: &mut [u64], from: usize, to: usize| {
            let mut tally = [0u64; 3];
            let mut sends = Vec::new();
            let sent = copy(
                (START, START + LEN),
                (from, to),
                prints,
                &mut scratch,
                &mut tally,
                |at, into| into.copy_from_slice(&title[at - START..at - START + into.len()]),
                |at, bytes| {
                    buffer[at - START..at - START + bytes.len()].copy_from_slice(bytes);
                    sends.push((at, at + bytes.len()));
                },
            );
            assert_eq!(&buffer[from - START..to - START], &title[from - START..to - START], "{from:#x}..{to:#x}");
            assert_eq!(tally.iter().sum::<u64>(), (to - from) as u64);
            assert_eq!(sent as u64, tally[Sent::Other as usize] + tally[Sent::Unknown as usize]);
            // In order; two touch only where a block read ends.
            assert!(
                sends.windows(2).all(|w| w[0].1 < w[1].0 || (w[0].1 == w[1].0 && w[0].1 % (256 << 10) == 0)),
                "{sends:?}"
            );
            (tally, sends)
        };
        // The first time, all of it is copied: nothing to compare with.
        let (tally, sends) = refresh(&title, &mut buffer, &mut prints, START, START + LEN);
        assert_eq!(tally, [0, 0, LEN as u64]);
        assert_eq!(sends.len(), (LEN / (256 << 10)), "one run per block read");
        // Nothing changed: nothing copied.
        let (tally, sends) = refresh(&title, &mut buffer, &mut prints, START, START + LEN);
        assert_eq!((tally, sends.len()), ([LEN as u64, 0, 0], 0));
        // Pages 3 and 9 written with other bytes, 4 to 8 rewritten with
        // their own.
        let page = |p: usize| p * PRINTED..(p + 1) * PRINTED;
        title[page(3)].iter_mut().for_each(|b| *b ^= 0x55);
        title[page(9).start + 4095] ^= 1;
        let (tally, sends) = refresh(&title, &mut buffer, &mut prints, START, START + 16 * PRINTED);
        assert_eq!(tally, [14 * PRINTED as u64, 2 * PRINTED as u64, 0]);
        let at = |p: usize| START + p * PRINTED;
        assert_eq!(sends, [(at(3), at(4)), (at(9), at(10))]);
        // A range that begins and ends inside pages: those are copied in
        // part and keep no print, so they are copied whole the next time
        // even though nothing changed. Page 21 changed and goes with the
        // part before it; page 22 is the same and left out.
        let (from, to) = (at(20) + 100, at(23) + 7);
        title[page(21)].iter_mut().for_each(|b| *b = b.wrapping_add(1));
        let (tally, sends) = refresh(&title, &mut buffer, &mut prints, from, to);
        assert_eq!(tally, [PRINTED as u64, PRINTED as u64, (PRINTED - 100 + 7) as u64]);
        assert_eq!(sends, [(from, at(22)), (at(23), to)]);
        let (tally, sends) = refresh(&title, &mut buffer, &mut prints, at(20), at(24));
        assert_eq!(tally, [2 * PRINTED as u64, 0, 2 * PRINTED as u64]);
        assert_eq!(sends, [(at(20), at(21)), (at(23), at(24))]);
        // Many rounds of writes, of bytes set back to what they held before
        // the last refresh, and of ranges, the buffer checked each time.
        let mut x = 0x1234_5678_9abc_def1u64;
        let mut next = |n: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n as u64) as usize
        };
        let mut undo: Vec<(usize, u8)> = Vec::new();
        for _ in 0..300 {
            for _ in 0..next(6) {
                let i = next(LEN);
                match next(3) {
                    0 => title[i] = title[i].wrapping_add(1 + next(255) as u8),
                    1 => {
                        undo.push((i, title[i]));
                        title[i] ^= 0x10;
                    }
                    _ => {
                        if let Some((i, was)) = undo.pop() {
                            title[i] = was;
                        }
                    }
                }
            }
            let from = START + next(LEN);
            let to = (from + 1 + next(300 << 10)).min(START + LEN);
            refresh(&title, &mut buffer, &mut prints, from, to);
        }
    }

    /// The pages compared with `sample` are about one in 16 of all, the
    /// same each time; their bytes count 16 times, and nothing compared
    /// changes what is copied.
    #[test]
    fn one_page_in_sixteen_is_compared_and_counted_sixteen_times() {
        let picked = (0..1 << 16).filter(|&p| sampled_page(p)).count();
        assert!((3600..=4600).contains(&picked), "{picked} of 65536");
        assert_eq!((0..1 << 16).filter(|&p| sampled_page(p)).count(), picked);
        const START: usize = 0x80_0000;
        const LEN: usize = 4 << 20;
        let title = memory(LEN, 9);
        let mut prints = vec![0u64; prints_for(START, START + LEN)];
        let read = |at: usize, into: &mut [u8]| {
            into.copy_from_slice(&title[at - START..at - START + into.len()]);
            true
        };
        let mut tally = [0u64; 3];
        sample((START, START + LEN), (START, START + LEN), &mut prints, &mut tally, read);
        let picked = (START / PRINTED..(START + LEN) / PRINTED).filter(|&p| sampled_page(p)).count() as u64;
        assert_eq!(tally, [0, 0, 16 * PRINTED as u64 * picked]);
        let mut tally = [0u64; 3];
        sample((START, START + LEN), (START, START + LEN), &mut prints, &mut tally, read);
        assert_eq!(tally, [16 * PRINTED as u64 * picked, 0, 0]);
        // A page not all there is not compared, and keeps no print.
        let p = (START / PRINTED..).find(|&p| sampled_page(p)).unwrap_or(0);
        let mut tally = [0u64; 3];
        let (from, to) = (p * PRINTED, (p + 1) * PRINTED);
        sample((START, START + LEN), (from, to), &mut prints, &mut tally, |_, _| false);
        assert_eq!(tally, [0, 0, 16 * PRINTED as u64]);
        assert_eq!(prints[p - START / PRINTED], 0);
    }
}
