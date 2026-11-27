//! Diagnostics: the live telemetry block the Kitty6 overlay reads
//! (`apps/kitty6-overlay`; layout in `victory_core::telemetry`).
//!
//! On in every diagnostics build, whatever `VICTORY_PROBE` says. The block
//! is a named file mapping made at the first present; from then on a
//! present costs a clock read and a few loads ([`tracking`]) unless the
//! overlay is up and asks for the numbers (F6 there turns that off and on).
//! While it asks, the probes' sampling runs (`probe`), each present adds
//! its time and page faults to the frame ring, and each second the probes'
//! numbers go into the block.
//!
//! Nothing here waits: a write that finds another under way is skipped,
//! and nothing allocates per frame.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use victory_core::telemetry::{Block, FrameRing, NAME, Second, Stall, Stats, Thread};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;

/// The mapped block, made once (`None`: the system refused; then nothing
/// here does anything).
fn block() -> Option<&'static Block> {
    static BLOCK: OnceLock<Option<usize>> = OnceLock::new();
    let at = (*BLOCK.get_or_init(|| map(NAME)))?;
    // SAFETY: a view of `size_of::<Block>()` bytes, page aligned, never unmapped.
    Some(unsafe { Block::from_ptr(at as *mut u8) })
}

/// Makes (or opens, when the overlay made it first) the mapping and
/// announces this process in it; the address of the view.
fn map(name: &str) -> Option<usize> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let size = size_of::<Block>() as u32;
    // SAFETY: a page-file backed mapping with a terminated name; the handle
    // is kept open for the life of the process (the view needs it).
    let mapping: HANDLE = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            PAGE_READWRITE,
            0,
            size,
            wide.as_ptr(),
        )
    };
    if mapping.is_null() {
        tracing::warn!("telemetry: no shared block for the overlay");
        return None;
    }
    // SAFETY: the mapping above, at least `size` bytes.
    let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, size as usize) };
    if view.Value.is_null() {
        tracing::warn!("telemetry: the overlay's block could not be mapped");
        return None;
    }
    // SAFETY: just mapped, page aligned, never unmapped.
    let block = unsafe { Block::from_ptr(view.Value.cast()) };
    // SAFETY: plain query.
    block.announce(unsafe { GetCurrentProcessId() });
    let switches = switches();
    // A block left by an earlier run (the overlay kept it open) starts over.
    block.frames.write(|f| *f = FrameRing::ZERO);
    block.stats.write(|s| {
        *s = Stats::ZERO;
        victory_core::telemetry::put_text(&mut s.switches, &switches);
        s.cpu_logical = std::thread::available_parallelism().map_or(0, |n| n.get() as u32);
    });
    tracing::info!(
        name,
        "telemetry block up for the Kitty6 overlay (kitty6-overlay.exe, F6)"
    );
    Some(view.Value as usize)
}

/// The `VICTORY_*` switches set, without the prefix; paths only by name.
fn switches() -> String {
    let mut all: Vec<String> = std::env::vars()
        .filter_map(|(k, v)| {
            let short = k.strip_prefix("VICTORY_")?.to_owned();
            let pathy = short.ends_with("_DIR")
                || matches!(short.as_str(), "CONFIG" | "PROBE")
                || v.contains('\\');
            Some(if pathy { short } else { format!("{short}={v}") })
        })
        .collect();
    all.sort();
    all.join(" ")
}

/// Whether the overlay is up and wants the numbers.
pub(crate) fn tracking() -> bool {
    let Some(b) = block() else { return false };
    // SAFETY: plain query (a read of shared user data).
    victory_core::telemetry::tracking_wanted(&b.header, unsafe { GetTickCount64() })
}

/// The window presented to, kept for the overlay to follow (a store only
/// when it changes). Makes the block on the first present.
pub(crate) fn window(hwnd: usize) {
    let Some(b) = block() else { return };
    if b.header.hwnd.load(Relaxed) != hwnd as u64 {
        b.header.hwnd.store(hwnd as u64, Relaxed);
    }
}

/// A writer is in the frame ring (presents from more than one thread
/// skip rather than wait).
static FRAME_WRITING: AtomicBool = AtomicBool::new(false);

/// A frame: its time and the page faults during it (presenting thread).
pub(crate) fn frame(ms: f32, faults: u32) {
    let Some(b) = block() else { return };
    if FRAME_WRITING.swap(true, std::sync::atomic::Ordering::Acquire) {
        return;
    }
    b.frames.write(|f: &mut FrameRing| f.push(ms, faults));
    FRAME_WRITING.store(false, std::sync::atomic::Ordering::Release);
}

/// What a second adds beside its numbers.
pub(crate) struct Extra<'a> {
    pub(crate) stall: Option<Stall>,
    pub(crate) threads: &'a [(String, f64)],
    pub(crate) groups: &'a [(String, f64)],
    pub(crate) stall_ms: f32,
}

/// A second (the meter's thread, the only writer of this section).
pub(crate) fn second(s: Second, extra: Extra<'_>) {
    let Some(b) = block() else { return };
    let (width, height) = crate::probe::resolution();
    let title_frame = crate::frame::current();
    b.stats.write(|st: &mut Stats| {
        st.push_second(s);
        if let Some(stall) = extra.stall {
            st.push_stall(stall);
        }
        for (out, list) in [
            (&mut st.threads, extra.threads),
            (&mut st.groups, extra.groups),
        ] {
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = list.get(i).map_or(Thread::ZERO, |(name, cores)| {
                    Thread::new(name, *cores as f32)
                });
            }
        }
        st.title_frame = title_frame;
        st.width = width;
        st.height = height;
        st.stall_ms = extra.stall_ms;
    });
    // SAFETY: plain query.
    b.header
        .game_tick_ms
        .store(unsafe { GetTickCount64() }, Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block maps (under a test name, not the game's), a reader sees
    /// what was written, and the header names this process.
    #[test]
    fn block_maps_and_reads_back() {
        let name = format!("Local\\Kitty6TelemetryTest{}", std::process::id());
        let at = map(&name).expect("mapped");
        // SAFETY: as in `block`.
        let b = unsafe { Block::from_ptr(at as *mut u8) };
        assert!(b.announced());
        assert_eq!(b.header.pid.load(Relaxed), std::process::id());
        b.frames.write(|f| f.push(33.3, 7));
        let f = b.frames.read(1).unwrap();
        assert_eq!(f.iter().last(), Some((33.3, 7)));
        let st = b.stats.read(1).unwrap();
        assert!(st.cpu_logical > 0);
        // A second view of the same name sees the same memory (the overlay's);
        // announcing again starts the sections over.
        let other = map(&name).expect("opened again");
        // SAFETY: as above.
        let o = unsafe { Block::from_ptr(other as *mut u8) };
        assert_eq!(o.frames.read(1).unwrap().count, 0);
        o.frames.write(|f| f.push(1.0, 1));
        assert_eq!(
            b.frames.read(1).unwrap().count,
            1,
            "both views share the memory"
        );
    }

    #[test]
    fn switches_leave_paths_out() {
        // SAFETY: tests here set only variables of their own.
        unsafe {
            std::env::set_var("VICTORY_TELEMETRY_TEST", "1");
            std::env::set_var("VICTORY_TELEMETRY_TEST_DIR", "C:\\somewhere");
        }
        let s = switches();
        assert!(s.contains("TELEMETRY_TEST=1"), "{s}");
        assert!(
            s.contains("TELEMETRY_TEST_DIR") && !s.contains("somewhere"),
            "{s}"
        );
    }
}
