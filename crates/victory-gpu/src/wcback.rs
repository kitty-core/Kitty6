//! `VICTORY_SKIP_WC_BACK=1` (off by default): what shaders wrote to a card
//! copy of title memory the title reserved write-combining is left on the
//! card, not fetched back into title memory, but for small declared ranges
//! (`VICTORY_SKIP_WC_KEEP`). After Peppacodes/playablegames-peppa PR 2 and
//! PR 17 (its `census` module; only the part that changes behaviour is
//! here). A census there found 97 of 100 MB a frame fetched back from such
//! memory; with a 1 MiB keep, -3.9 ms a frame on an RTX 5080.
//!
//! Risk: where the title's CPU writes part of a 64 KiB piece the card
//! wrote, the next upload sends stale bytes there; and the title's CPU
//! reads of large write-combining ranges see old bytes. The source's
//! `VICTORY_SKIP_WC_COMPARE` (not even comparing those ranges on the card)
//! had a device removal reported with it and is not ported.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// The console's `PAGE_WRITECOMBINE` in what the title asked for.
const WRITE_COMBINE: u32 = 0x400;

/// Reservations of `XMemVirtualAlloc` by start: their length and the
/// protection the title asked for (before the runtime cleans it).
static RESERVATIONS: Mutex<BTreeMap<usize, (usize, u32)>> = Mutex::new(BTreeMap::new());

/// The runtime reserved `[at, at + len)` for the title (`XMemVirtualAlloc`
/// at an address of its choice) with the console `protect` it asked for.
/// Kept only with [`skip_wc_back`].
pub fn title_memory_reserved(at: usize, len: usize, protect: u32) {
    if !skip_wc_back() || at == 0 || len == 0 {
        return;
    }
    RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(at, (len, protect));
}

/// Whether `VICTORY_SKIP_WC_BACK=1`.
fn skip_wc_back() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_SKIP_WC_BACK").is_some_and(|v| v == "1");
        if on {
            tracing::warn!(
                "VICTORY_SKIP_WC_BACK=1: shader writes to write-combining title memory larger than {} bytes stay on the card",
                keep()
            );
        }
        on
    })
}

/// `VICTORY_SKIP_WC_KEEP=<bytes>`: declared ranges of at most that many
/// bytes are still fetched back, for the small results the title reads on
/// the CPU (the picture went black without them there: exposure, likely).
/// Here 1 MiB when unset, the value the source measured with (its default
/// was 0, which breaks the picture); 0: none kept.
fn keep() -> usize {
    static KEEP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *KEEP.get_or_init(|| std::env::var("VICTORY_SKIP_WC_KEEP").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(1 << 20))
}

/// Every range lies wholly inside a single write-combining reservation.
fn all_wc_in(reservations: &BTreeMap<usize, (usize, u32)>, ranges: &[(usize, usize)]) -> bool {
    !ranges.is_empty()
        && ranges.iter().all(|&(from, to)| {
            // Seeking from the exclusive end also finds a reservation
            // beginning inside the range. One ending at `to` fits.
            from < to
                && reservations.range(..to).next_back().is_some_and(|(&base, &(len, protect))| {
                    base <= from && protect & WRITE_COMBINE != 0 && base.checked_add(len).is_some_and(|end| to <= end)
                })
        })
}

/// Pieces left on the card: count and bytes.
static SKIPPED: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// Whether a flagged piece of `bytes` of the declared range `declared`
/// (title addresses) is left on the card instead of fetched back. A piece
/// of a declaration not wholly in write-combining memory is fetched.
pub(crate) fn skip_back(declared: (usize, usize), bytes: u64) -> bool {
    if !skip_wc_back() {
        return false;
    }
    if declared.1.saturating_sub(declared.0) <= keep() {
        return false;
    }
    // In the in-run A/B (diagnostics builds; the variable must still be set,
    // for the reservations to be known): off, everything is fetched again.
    #[cfg(feature = "diagnostics")]
    if crate::ab::pick("VICTORY_SKIP_WC_BACK") == Some(false) {
        return false;
    }
    let wc = all_wc_in(&RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner()), &[declared]);
    if wc {
        use std::sync::atomic::Ordering;
        let n = SKIPPED[0].fetch_add(1, Ordering::Relaxed) + 1;
        let b = SKIPPED[1].fetch_add(bytes, Ordering::Relaxed) + bytes;
        if n.is_power_of_two() {
            tracing::info!("VICTORY_SKIP_WC_BACK: {n} pieces ({} MB) of write-combining memory left on the card so far", b >> 20);
        }
    }
    wc
}

#[cfg(test)]
mod tests {
    use super::all_wc_in;
    use std::collections::BTreeMap;

    #[test]
    fn a_write_in_the_ordinary_half_of_a_card_window_still_comes_back() {
        const MIB: usize = 1 << 20;
        let window = 8 * MIB;
        let middle = window + 2 * MIB;
        let map = BTreeMap::from([(window, (2 * MIB, 0x404)), (middle, (2 * MIB, 0x04))]);
        let wc = (middle - 65536, middle);
        let ordinary = (middle, middle + 65536);
        assert!(all_wc_in(&map, &[wc]));
        assert!(!all_wc_in(&map, &[ordinary]));
        assert!(!all_wc_in(&map, &[wc, ordinary]));
        assert!(!all_wc_in(&map, &[(wc.0, ordinary.1)]));
    }

    #[test]
    fn a_declaration_must_fit_one_known_reservation() {
        let map = BTreeMap::from([(0x10000, (0x10000, 0x404)), (0x20000, (0x10000, 0x404)), (usize::MAX - 8, (16, 0x404))]);
        assert!(all_wc_in(&map, &[(0x10000, 0x20000), (0x20000, 0x30000)]));
        assert!(!all_wc_in(&map, &[(0x1ffff, 0x20001)]));
        assert!(!all_wc_in(&map, &[(0x30000, 0x30001)]));
        assert!(!all_wc_in(&map, &[(0x10000, 0x10000)]));
        assert!(!all_wc_in(&map, &[(usize::MAX - 8, 1)]));
        assert!(!all_wc_in(&map, &[(usize::MAX - 8, usize::MAX)]));
    }
}
