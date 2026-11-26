//! The live telemetry block shared between a diagnostics build of the game
//! (the writer, `victory_gpu`'s `telemetry` module) and the Kitty6 overlay
//! (`apps/kitty6-overlay`, the reader).
//!
//! The block is a named file mapping ([`NAME`]) holding one [`Block`]: a
//! header of plain atomics, then two sections each behind its own seqlock
//! ([`Seqlocked`]): the ring of the last [`FRAMES`] frame times (written by
//! the presenting thread) and the per-second numbers (written by the meter's
//! thread). One writer per section; a reader copies a section and keeps the
//! copy only if the sequence counter was even and unchanged around it.
//!
//! The overlay asks for the heavy sampling with [`Header::tracking`] and
//! proves it is alive with [`Header::overlay_tick_ms`]; the game samples
//! only while [`tracking_wanted`] says so.
//!
//! Everything here is plain data with a fixed layout: no pointers, no
//! allocation, the same in both processes.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

/// The mapping's name (per session).
pub const NAME: &str = "Local\\Kitty6Telemetry";
/// The name `kitty6-overlay --demo` publishes fake data under, so a demo
/// never writes into a running game's block.
pub const DEMO_NAME: &str = "Local\\Kitty6TelemetryDemo";
/// `K6TM`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"K6TM");
/// Bumped whenever the layout changes.
pub const VERSION: u32 = 1;
/// Frames kept in the ring.
pub const FRAMES: usize = 512;
/// Seconds kept for the sparklines.
pub const SECONDS: usize = 120;
/// Stalls kept.
pub const STALLS: usize = 8;
/// Threads (and kinds of thread) kept, busiest first.
pub const THREADS: usize = 6;
/// Bytes of a thread's name.
pub const NAME_LEN: usize = 32;
/// Bytes of the switches text.
pub const SWITCHES_LEN: usize = 1024;
/// The overlay's heartbeat older than this (ms) and the game stops sampling.
pub const OVERLAY_TIMEOUT_MS: u64 = 3000;

/// A section behind a seqlock: `seq` odd while it is written.
#[repr(C)]
pub struct Seqlocked<T: Copy> {
    seq: AtomicU32,
    _pad: [u32; 15],
    data: UnsafeCell<T>,
}

// SAFETY: writes are bracketed by the sequence counter and readers keep a
// copy only when it shows no write overlapped (see `read`).
unsafe impl<T: Copy> Sync for Seqlocked<T> {}

impl<T: Copy> Seqlocked<T> {
    /// The counter (even: settled), for tests and for telling new data.
    pub fn seq(&self) -> u32 {
        self.seq.load(Ordering::Acquire)
    }

    /// Changes the section in place. One writer at a time per section: the
    /// caller makes sure of that (the game's publisher skips a write when
    /// another is under way rather than waiting).
    pub fn write(&self, f: impl FnOnce(&mut T)) {
        self.seq.fetch_add(1, Ordering::AcqRel);
        fence(Ordering::Release);
        // SAFETY: the only writer; readers that overlap see an odd or a
        // changed counter and throw their copy away.
        f(unsafe { &mut *self.data.get() });
        fence(Ordering::Release);
        self.seq.fetch_add(1, Ordering::Release);
    }

    /// A consistent copy, or `None` after `tries` overlapping writes.
    pub fn read(&self, tries: u32) -> Option<T> {
        for _ in 0..tries {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            // SAFETY: plain old data; a torn copy is detected below and dropped.
            let copy = unsafe { std::ptr::read_volatile(self.data.get()) };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return Some(copy);
            }
        }
        None
    }

    /// A copy into `out` (for large sections, no copy on the stack);
    /// false after `tries` overlapping writes.
    pub fn read_into(&self, out: &mut T, tries: u32) -> bool {
        for _ in 0..tries {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            // SAFETY: as in `read`; `out` is ours.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.data.get().cast::<u8>().cast_const(),
                    (out as *mut T).cast::<u8>(),
                    size_of::<T>(),
                );
            }
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return true;
            }
        }
        false
    }
}

/// The block's first bytes.
#[repr(C)]
pub struct Header {
    /// [`MAGIC`] once the game has filled in the header.
    pub magic: AtomicU32,
    /// [`VERSION`].
    pub version: AtomicU32,
    /// `size_of::<Block>()`.
    pub size: AtomicU32,
    /// The game's process id.
    pub pid: AtomicU32,
    /// Set by the overlay: 1, sample the heavy numbers; 0, don't.
    pub tracking: AtomicU32,
    pub _pad: AtomicU32,
    /// The overlay's `GetTickCount64` at its last look.
    pub overlay_tick_ms: AtomicU64,
    /// The game's `GetTickCount64` at its last write.
    pub game_tick_ms: AtomicU64,
    /// The window the game presents to (an `HWND`).
    pub hwnd: AtomicU64,
}

/// Whether the game should sample: the overlay asked and was alive lately.
pub fn tracking_wanted(header: &Header, now_ms: u64) -> bool {
    header.tracking.load(Ordering::Relaxed) != 0
        && now_ms.saturating_sub(header.overlay_tick_ms.load(Ordering::Relaxed))
            < OVERLAY_TIMEOUT_MS
}

/// The last frames: present to present in ms, and the process's page faults
/// during each. Frame `n` (counting from 0) is at `n % FRAMES`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrameRing {
    /// Frames written so far.
    pub count: u64,
    pub ms: [f32; FRAMES],
    pub faults: [u32; FRAMES],
}

impl FrameRing {
    pub const ZERO: FrameRing = FrameRing {
        count: 0,
        ms: [0.0; FRAMES],
        faults: [0; FRAMES],
    };

    /// Adds a frame.
    pub fn push(&mut self, ms: f32, faults: u32) {
        let at = (self.count % FRAMES as u64) as usize;
        self.ms[at] = ms;
        self.faults[at] = faults;
        self.count += 1;
    }

    /// The kept frames, oldest first: (ms, faults).
    pub fn iter(&self) -> impl Iterator<Item = (f32, u32)> + '_ {
        let kept = self.count.min(FRAMES as u64);
        (self.count - kept..self.count).map(|n| {
            let at = (n % FRAMES as u64) as usize;
            (self.ms[at], self.faults[at])
        })
    }
}

/// A second's numbers (all `f32`: counts, ms, MB, rates).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Second {
    pub t_s: f32,
    pub frames: f32,
    pub fps: f32,
    pub ms_mean: f32,
    pub ms_p50: f32,
    pub ms_p95: f32,
    pub ms_max: f32,
    pub over33: f32,
    pub over100: f32,
    pub over500: f32,
    pub prep_ms: f32,
    pub gpu_ms: f32,
    pub draws: f32,
    pub dropped: f32,
    pub pipelines_waiting: f32,
    pub pipelines_made: f32,
    pub pipeline_ms_mean: f32,
    pub pipeline_ms_max: f32,
    pub pipeline_wait_ms: f32,
    pub pipelines_refused: f32,
    pub ws_mb: f32,
    pub private_mb: f32,
    pub faults_per_s: f32,
    pub io_read_mb_s: f32,
    pub sys_pages_in_per_s: f32,
    pub avail_ram_mb: f32,
    pub total_ram_mb: f32,
    pub avail_commit_mb: f32,
    pub commit_limit_mb: f32,
    pub mem_load_pct: f32,
    pub vram_budget_mb: f32,
    pub vram_used_mb: f32,
    pub vram_shared_mb: f32,
    pub cpu_cores: f32,
    pub busiest_thread_cores: f32,
    /// Host copies of title memory alive, and their MB.
    pub host_copies: f32,
    pub host_copies_mb: f32,
    /// Host textures alive, and their MB.
    pub host_textures: f32,
    pub host_textures_mb: f32,
    /// Staging buffers of textures alive, MB.
    pub staging_mb: f32,
    /// Host lists' other upload buffers alive, and their MB.
    pub upload_buffers: f32,
    pub upload_mb: f32,
    /// Host lists' scratch memory alive, MB.
    pub scratch_mb: f32,
    pub texture_uploads_per_s: f32,
    /// Command allocators made anew so far (`VICTORY_ALLOCATOR_RENEW`).
    pub allocators_renewed: f32,
    /// Texture levels left out so far.
    pub left_out_levels: f32,
}

/// The slowest frame of a second that took the stall time or more.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stall {
    pub t_s: f32,
    pub ms: f32,
    pub faults: f32,
    pub pipelines_made: f32,
    pub pipelines_waiting: f32,
    pub sys_pages_in: f32,
}

/// A thread, or a kind of thread, and the cores it kept busy.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thread {
    pub name: [u8; NAME_LEN],
    pub cores: f32,
}

impl Thread {
    pub const ZERO: Thread = Thread {
        name: [0; NAME_LEN],
        cores: 0.0,
    };

    pub fn new(name: &str, cores: f32) -> Thread {
        let mut t = Thread {
            cores,
            ..Thread::ZERO
        };
        put_text(&mut t.name, name);
        t
    }

    pub fn name(&self) -> &str {
        text(&self.name)
    }
}

/// The per-second section.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Stats {
    /// Seconds written so far; the newest is `history[(seconds - 1) % SECONDS]`.
    pub seconds: u64,
    pub now: Second,
    pub history: [Second; SECONDS],
    /// Stalls so far; the newest is `stalls[(stall_count - 1) % STALLS]`.
    pub stall_count: u64,
    pub stalls: [Stall; STALLS],
    pub threads: [Thread; THREADS],
    pub groups: [Thread; THREADS],
    /// The title's frame number (its frame waits that ended).
    pub title_frame: u64,
    /// The swap chain's size.
    pub width: u32,
    pub height: u32,
    /// Logical processors of the PC.
    pub cpu_logical: u32,
    /// The stall time in ms (`VICTORY_PROBE_STALL_MS`).
    pub stall_ms: f32,
    /// The `VICTORY_*` switches set, `NAME=value` separated by spaces,
    /// without the prefix.
    pub switches: [u8; SWITCHES_LEN],
}

impl Stats {
    pub const ZERO: Stats = Stats {
        seconds: 0,
        now: Second::ZERO,
        history: [Second::ZERO; SECONDS],
        stall_count: 0,
        stalls: [Stall::ZERO; STALLS],
        threads: [Thread::ZERO; THREADS],
        groups: [Thread::ZERO; THREADS],
        title_frame: 0,
        width: 0,
        height: 0,
        cpu_logical: 0,
        stall_ms: 100.0,
        switches: [0; SWITCHES_LEN],
    };

    /// Records a second: `now`, and the history.
    pub fn push_second(&mut self, s: Second) {
        self.now = s;
        self.history[(self.seconds % SECONDS as u64) as usize] = s;
        self.seconds += 1;
    }

    /// Records a stall.
    pub fn push_stall(&mut self, s: Stall) {
        self.stalls[(self.stall_count % STALLS as u64) as usize] = s;
        self.stall_count += 1;
    }

    /// The kept seconds, oldest first.
    pub fn history(&self) -> impl Iterator<Item = &Second> + '_ {
        let kept = self.seconds.min(SECONDS as u64);
        (self.seconds - kept..self.seconds).map(|n| &self.history[(n % SECONDS as u64) as usize])
    }

    /// The kept stalls, newest first.
    pub fn stalls(&self) -> impl Iterator<Item = &Stall> + '_ {
        let kept = self.stall_count.min(STALLS as u64);
        (0..kept).map(move |i| &self.stalls[((self.stall_count - 1 - i) % STALLS as u64) as usize])
    }

    pub fn switches(&self) -> &str {
        text(&self.switches)
    }
}

impl Second {
    pub const ZERO: Second = Second {
        t_s: 0.0,
        frames: 0.0,
        fps: 0.0,
        ms_mean: 0.0,
        ms_p50: 0.0,
        ms_p95: 0.0,
        ms_max: 0.0,
        over33: 0.0,
        over100: 0.0,
        over500: 0.0,
        prep_ms: 0.0,
        gpu_ms: 0.0,
        draws: 0.0,
        dropped: 0.0,
        pipelines_waiting: 0.0,
        pipelines_made: 0.0,
        pipeline_ms_mean: 0.0,
        pipeline_ms_max: 0.0,
        pipeline_wait_ms: 0.0,
        pipelines_refused: 0.0,
        ws_mb: 0.0,
        private_mb: 0.0,
        faults_per_s: 0.0,
        io_read_mb_s: 0.0,
        sys_pages_in_per_s: 0.0,
        avail_ram_mb: 0.0,
        total_ram_mb: 0.0,
        avail_commit_mb: 0.0,
        commit_limit_mb: 0.0,
        mem_load_pct: 0.0,
        vram_budget_mb: 0.0,
        vram_used_mb: 0.0,
        vram_shared_mb: 0.0,
        cpu_cores: 0.0,
        busiest_thread_cores: 0.0,
        host_copies: 0.0,
        host_copies_mb: 0.0,
        host_textures: 0.0,
        host_textures_mb: 0.0,
        staging_mb: 0.0,
        upload_buffers: 0.0,
        upload_mb: 0.0,
        scratch_mb: 0.0,
        texture_uploads_per_s: 0.0,
        allocators_renewed: 0.0,
        left_out_levels: 0.0,
    };
}

impl Stall {
    pub const ZERO: Stall = Stall {
        t_s: 0.0,
        ms: 0.0,
        faults: 0.0,
        pipelines_made: 0.0,
        pipelines_waiting: 0.0,
        sys_pages_in: 0.0,
    };
}

/// The whole mapping.
#[repr(C)]
pub struct Block {
    pub header: Header,
    pub frames: Seqlocked<FrameRing>,
    pub stats: Seqlocked<Stats>,
}

impl Block {
    /// The header filled in by the game (keeps the overlay's fields).
    pub fn announce(&self, pid: u32) {
        self.header.version.store(VERSION, Ordering::Relaxed);
        self.header
            .size
            .store(size_of::<Block>() as u32, Ordering::Relaxed);
        self.header.pid.store(pid, Ordering::Relaxed);
        self.header.magic.store(MAGIC, Ordering::Release);
    }

    /// Whether the game announced this layout.
    pub fn announced(&self) -> bool {
        self.header.magic.load(Ordering::Acquire) == MAGIC
            && self.header.version.load(Ordering::Relaxed) == VERSION
            && self.header.size.load(Ordering::Relaxed) == size_of::<Block>() as u32
    }

    /// A block over `bytes` (a fresh mapping is all zeros, which is a valid
    /// empty block).
    ///
    /// # Safety
    /// `bytes` points to at least `size_of::<Block>()` bytes, aligned to 8,
    /// that live as long as the returned reference and are shared only
    /// through `Block`s.
    pub unsafe fn from_ptr<'a>(bytes: *mut u8) -> &'a Block {
        debug_assert_eq!(bytes as usize % 8, 0);
        // SAFETY: by the caller; every field is valid for any bit pattern
        // apart from the `f32`s, which are valid for any too.
        unsafe { &*(bytes.cast::<Block>()) }
    }
}

/// Copies `s` into `out`, cut at a character boundary, NUL-padded.
pub fn put_text(out: &mut [u8], s: &str) {
    let mut end = s.len().min(out.len().saturating_sub(1));
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    out[..end].copy_from_slice(&s.as_bytes()[..end]);
    out[end..].fill(0);
}

/// The text in a NUL-padded buffer (empty if not UTF-8).
pub fn text(bytes: &[u8]) -> &str {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of};

    /// The layout both processes rely on: fixed offsets, 8-byte alignment,
    /// sections on their own cache lines.
    #[test]
    fn layout_is_fixed() {
        assert_eq!(size_of::<Header>(), 48);
        assert_eq!(offset_of!(Header, overlay_tick_ms), 24);
        assert_eq!(offset_of!(Header, hwnd), 40);
        assert_eq!(offset_of!(Block, frames), 48);
        assert_eq!(offset_of!(Seqlocked<FrameRing>, data), 64);
        assert_eq!(size_of::<FrameRing>(), 8 + FRAMES * 8);
        assert_eq!(size_of::<Second>(), 46 * 4);
        assert_eq!(size_of::<Stall>(), 24);
        assert_eq!(size_of::<Thread>(), NAME_LEN + 4);
        assert_eq!(align_of::<Block>(), 8);
        assert_eq!(offset_of!(Block, stats) % 8, 0);
        // Small enough to copy 30 times a second.
        assert!(size_of::<Block>() < 64 * 1024, "{}", size_of::<Block>());
    }

    fn zeroed_block() -> Box<[u64]> {
        vec![0u64; size_of::<Block>().div_ceil(8)].into_boxed_slice()
    }

    #[test]
    fn zeroed_memory_is_an_empty_block() {
        let mut mem = zeroed_block();
        // SAFETY: an aligned zeroed buffer of the right size, alive below.
        let block = unsafe { Block::from_ptr(mem.as_mut_ptr().cast()) };
        assert!(!block.announced());
        assert_eq!(block.frames.read(1).unwrap().count, 0);
        block.announce(42);
        assert!(block.announced());
        assert_eq!(block.header.pid.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn seqlock_counts_and_copies() {
        let mut mem = zeroed_block();
        // SAFETY: as above.
        let block = unsafe { Block::from_ptr(mem.as_mut_ptr().cast()) };
        assert_eq!(block.frames.seq(), 0);
        block.frames.write(|f| f.push(16.6, 3));
        assert_eq!(
            block.frames.seq(),
            2,
            "even after a write, two steps a write"
        );
        let f = block.frames.read(1).unwrap();
        assert_eq!(f.count, 1);
        assert_eq!(f.iter().collect::<Vec<_>>(), vec![(16.6, 3)]);
        // A write under way: the reader gives up rather than return a torn copy.
        block.frames.seq.fetch_add(1, Ordering::AcqRel);
        assert!(block.frames.read(3).is_none());
        let mut out = FrameRing::ZERO;
        assert!(!block.frames.read_into(&mut out, 3));
        block.frames.seq.fetch_add(1, Ordering::AcqRel);
        assert!(block.frames.read_into(&mut out, 1));
        assert_eq!(out.count, 1);
    }

    /// A reader racing a writer only ever sees whole writes.
    #[test]
    fn seqlock_never_tears() {
        let mut mem = zeroed_block();
        let at = mem.as_mut_ptr() as usize;
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                // SAFETY: as above; the buffer outlives the scope.
                let block = unsafe { Block::from_ptr(at as *mut u8) };
                for n in 1..=20_000u32 {
                    block.frames.write(|f| {
                        f.ms.fill(n as f32);
                        f.faults.fill(n);
                    });
                }
                stop.store(true, Ordering::Relaxed);
            });
            // SAFETY: as above.
            let block = unsafe { Block::from_ptr(at as *mut u8) };
            let mut seen = 0;
            let mut out = FrameRing::ZERO;
            while !stop.load(Ordering::Relaxed) {
                if block.frames.read_into(&mut out, 100) {
                    let first = out.faults[0];
                    assert!(out.faults.iter().all(|&v| v == first), "torn copy");
                    assert!(out.ms.iter().all(|&v| v == first as f32), "torn copy");
                    seen += 1;
                }
            }
            assert!(seen > 0);
        });
    }

    #[test]
    fn rings_wrap() {
        let mut f = FrameRing::ZERO;
        for i in 0..(FRAMES + 10) {
            f.push(i as f32, i as u32);
        }
        let kept: Vec<_> = f.iter().collect();
        assert_eq!(kept.len(), FRAMES);
        assert_eq!(kept[0], (10.0, 10));
        assert_eq!(kept[FRAMES - 1].1, (FRAMES + 9) as u32);

        let mut s = Box::new(Stats::ZERO);
        for i in 0..(SECONDS + 5) {
            s.push_second(Second {
                t_s: i as f32,
                ..Second::ZERO
            });
        }
        let h: Vec<f32> = s.history().map(|x| x.t_s).collect();
        assert_eq!(h.len(), SECONDS);
        assert_eq!(h[0], 5.0);
        assert_eq!(s.now.t_s, (SECONDS + 4) as f32);
        for i in 0..10 {
            s.push_stall(Stall {
                ms: i as f32,
                ..Stall::ZERO
            });
        }
        let st: Vec<f32> = s.stalls().map(|x| x.ms).collect();
        assert_eq!(st, vec![9.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0]);
    }

    #[test]
    fn tracking_needs_a_live_overlay() {
        let mut mem = zeroed_block();
        // SAFETY: as above.
        let block = unsafe { Block::from_ptr(mem.as_mut_ptr().cast()) };
        let h = &block.header;
        assert!(!tracking_wanted(h, 10_000));
        h.tracking.store(1, Ordering::Relaxed);
        h.overlay_tick_ms.store(10_000, Ordering::Relaxed);
        assert!(tracking_wanted(h, 10_500));
        assert!(
            !tracking_wanted(h, 10_000 + OVERLAY_TIMEOUT_MS),
            "a dead overlay stops the sampling"
        );
        h.tracking.store(0, Ordering::Relaxed);
        assert!(!tracking_wanted(h, 10_500));
    }

    #[test]
    fn texts_are_cut_and_padded() {
        let mut b = [0xffu8; 8];
        put_text(&mut b, "victory-thin-12");
        assert_eq!(text(&b), "victory");
        put_text(&mut b, "aé");
        assert_eq!(text(&b), "aé");
        put_text(&mut b, "abcdeéé");
        assert_eq!(text(&b), "abcdeé");
        put_text(&mut b, "abcdefé");
        assert_eq!(text(&b), "abcdef", "cut before a split character");
        assert_eq!(Thread::new("x", 0.5).name(), "x");
    }
}
