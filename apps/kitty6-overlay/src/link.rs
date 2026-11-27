//! The overlay's end of the telemetry block: the mapping (made or opened,
//! whichever process comes first), consistent copies of it, and whether the
//! game that announced itself is still running.

use std::sync::atomic::Ordering::Relaxed;

use victory_core::telemetry::{Block, FrameRing, Header, Stats};
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE,
};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::PCWSTR;

pub struct Link {
    block: &'static Block,
    _mapping: HANDLE,
}

impl Link {
    /// Makes or opens the mapping `name`.
    pub fn open(name: &str) -> windows::core::Result<Link> {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let size = size_of::<Block>() as u32;
        // SAFETY: a page-file backed mapping, kept open while the link lives
        // (the process's life).
        let mapping = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                size,
                PCWSTR(wide.as_ptr()),
            )?
        };
        // SAFETY: the mapping above, at least `size` bytes.
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, size as usize) };
        if view.Value.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        // SAFETY: page aligned, never unmapped.
        let block = unsafe { Block::from_ptr(view.Value.cast()) };
        Ok(Link {
            block,
            _mapping: mapping,
        })
    }

    pub fn block(&self) -> &'static Block {
        self.block
    }

    pub fn header(&self) -> &Header {
        &self.block.header
    }

    /// Asks for (or stops) the sampling, and says the overlay is alive.
    pub fn beat(&self, tracking: bool) {
        let h = self.header();
        h.tracking.store(u32::from(tracking), Relaxed);
        h.overlay_tick_ms.store(now_ms(), Relaxed);
    }

    /// The game announced in the block, if it still runs: its pid.
    pub fn game(&self) -> Option<u32> {
        if !self.block.announced() {
            return None;
        }
        let pid = self.header().pid.load(Relaxed);
        alive(pid).then_some(pid)
    }

    /// Copies both sections; false if a writer kept getting in the way.
    pub fn read(&self, frames: &mut FrameRing, stats: &mut Stats) -> bool {
        self.block.frames.read_into(frames, 50) & self.block.stats.read_into(stats, 50)
    }
}

pub fn now_ms() -> u64 {
    // SAFETY: plain query.
    unsafe { GetTickCount64() }
}

/// Whether process `pid` runs.
pub fn alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: a query-only handle we close.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(process, &mut code).is_ok();
        let _ = CloseHandle(process);
        ok && code == STILL_ACTIVE.0 as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_links_share_a_block() {
        let name = format!("Local\\Kitty6OverlayTest{}", std::process::id());
        let a = Link::open(&name).unwrap();
        let b = Link::open(&name).unwrap();
        assert!(a.game().is_none(), "nobody announced");
        a.block().announce(std::process::id());
        assert_eq!(b.game(), Some(std::process::id()), "we are alive");
        a.block().frames.write(|f| f.push(20.0, 1));
        let mut frames = FrameRing::ZERO;
        let mut stats = Box::new(Stats::ZERO);
        assert!(b.read(&mut frames, &mut stats));
        assert_eq!(frames.count, 1);
        b.beat(true);
        assert!(victory_core::telemetry::tracking_wanted(
            a.header(),
            now_ms()
        ));
        b.beat(false);
        assert!(!victory_core::telemetry::tracking_wanted(
            a.header(),
            now_ms()
        ));
        assert!(!alive(0));
    }
}
