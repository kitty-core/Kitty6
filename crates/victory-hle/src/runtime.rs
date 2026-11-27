//! Process-wide state the implementations need.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

static MAIN_IMAGE_BASE: AtomicUsize = AtomicUsize::new(0);

/// Records where the title's main image is mapped. The title expects
/// `GetModuleHandle(NULL)` and friends to return this, not our own exe.
pub fn set_main_image(base: usize) {
    MAIN_IMAGE_BASE.store(base, Ordering::Release);
    crate::image_watch::start(base);
}

pub fn main_image() -> Option<usize> {
    match MAIN_IMAGE_BASE.load(Ordering::Acquire) {
        0 => None,
        base => Some(base),
    }
}

/// Original file of each title image we mapped, by base address.
static IMAGE_PATHS: Mutex<Vec<(usize, PathBuf)>> = Mutex::new(Vec::new());

/// Records the file a title image was mapped from: the title asks for its
/// own path (`GetModuleFileName`) to find its data, and must not get our
/// cache copy.
pub fn set_image_path(base: usize, path: &Path) {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let mut paths = IMAGE_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    paths.retain(|(b, _)| *b != base);
    paths.push((base, path));
}

/// Forgets a title image that was unmapped again.
pub fn forget_image(base: usize) {
    let mut paths = IMAGE_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    paths.retain(|(b, _)| *b != base);
}

pub fn image_path(base: usize) -> Option<PathBuf> {
    let paths = IMAGE_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    paths
        .iter()
        .find(|(b, _)| *b == base)
        .map(|(_, p)| p.clone())
}

/// The title image whose original file is called `file` (any case).
pub fn image_named(file: &str) -> Option<usize> {
    let paths = IMAGE_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    paths
        .iter()
        .find(|(_, p)| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(file)))
        .map(|(b, _)| *b)
}

static TITLE_ARGS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// The arguments the title's `main` gets (those after `--`). Set once, before
/// the title starts; later calls are ignored.
pub fn set_title_args(args: Vec<String>) {
    let _ = TITLE_ARGS.set(args);
}

pub fn title_args() -> &'static [String] {
    TITLE_ARGS.get().map_or(&[], Vec::as_slice)
}

/// Maps a title DLL the title loads itself (`LoadLibrary`) and returns its
/// base; `Ok(None)` if `file` is not one of the title's DLLs.
pub type TitleDllLoader = Box<dyn Fn(&str) -> Result<Option<usize>, String> + Send + Sync>;

static TITLE_DLL_LOADER: std::sync::OnceLock<TitleDllLoader> = std::sync::OnceLock::new();

/// Set once, by whoever mapped the title (it knows the folders and the
/// resolver); later calls are ignored.
pub fn set_title_dll_loader(loader: TitleDllLoader) {
    let _ = TITLE_DLL_LOADER.set(loader);
}

pub fn load_title_dll(file: &str) -> Result<Option<usize>, String> {
    TITLE_DLL_LOADER.get().map_or(Ok(None), |load| load(file))
}

/// What the title's manifest (`MicrosoftGame.config` beside its executable)
/// says about it, and where its persistent local storage lives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Title {
    pub id: u32,
    /// The store's product id (empty if the manifest has none).
    pub store_id: String,
    /// Persistent local storage: our folder for it and the title's quota.
    /// Game saves live beside it ([`Title::saves`]).
    pub storage: PathBuf,
    pub storage_bytes: u64,
}

impl Title {
    /// Id, store id and storage quota from the manifest (zero or empty
    /// where it has none); the storage folder is left for the caller.
    pub fn from_manifest(text: &str) -> Title {
        let elements = elements(text);
        let get = |path: &str| {
            elements
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, text)| text.as_str())
        };
        let id = get("Game/TitleId").and_then(|v| u32::from_str_radix(v, 16).ok());
        let storage_mb = get("Game/PersistentLocalStorage/SizeMB").and_then(|v| v.parse().ok());
        Title {
            id: id.unwrap_or(0),
            store_id: get("Game/StoreId").unwrap_or_default().to_owned(),
            storage: PathBuf::new(),
            storage_bytes: storage_mb.unwrap_or(0) << 20,
        }
    }

    /// Where game saves live: next to persistent local storage, under the
    /// data directory, never the cache.
    pub fn saves(&self) -> PathBuf {
        self.storage.with_file_name("saves")
    }
}

/// The trimmed text of one element: comments inside it are skipped, not
/// treated as the end of its value. A manifest generator can write
/// `<TitleId><!-- generated -->639AA8DD</TitleId>`, which must still give
/// the title's id rather than an empty field.
fn field_text(rest: &str) -> String {
    let mut out = String::new();
    let mut rest = rest;
    loop {
        let Some(open) = rest.find('<') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]);
        if rest[open..].starts_with("<!--") {
            let Some(end) = rest[open..].find("-->") else {
                break;
            };
            rest = &rest[open + end + 3..];
        } else {
            // A child element, a closing tag or anything else ends the text.
            break;
        }
    }
    out.trim().to_owned()
}

/// Each element of an XML text as its path from the root (`Game/TitleId`)
/// and its trimmed text up to the first child, the predefined entities
/// decoded; comments, CDATA sections, declarations and attributes are
/// skipped.
fn elements(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut path: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        let end = if rest.starts_with("<!--") {
            rest.find("-->").map(|e| e + 3)
        } else if rest.starts_with("<![CDATA[") {
            rest.find("]]>").map(|e| e + 3)
        } else {
            tag_end(rest)
        };
        let Some(end) = end else { break };
        let tag = &rest[1..end - 1];
        rest = &rest[end..];
        if tag.starts_with(['!', '?']) {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            if let Some(at) = path.iter().rposition(|n| *n == name.trim()) {
                path.truncate(at);
            }
            continue;
        }
        let closed = tag.ends_with('/');
        let name = tag
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");
        path.push(name);
        let body = field_text(rest);
        let body = [("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&apos;", "'"), ("&amp;", "&")]
            .iter()
            .fold(body, |b, (entity, c)| b.replace(entity, c));
        out.push((path.join("/"), body));
        if closed {
            path.pop();
        }
    }
    out
}

/// Where the tag that starts `text` ends, past its `>`; a `>` in a quoted
/// attribute value is part of the value (XML allows it there unescaped).
fn tag_end(text: &str) -> Option<usize> {
    let mut quote = None;
    for (at, c) in text.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '>') => return Some(at + 1),
            _ => {}
        }
    }
    None
}

static TITLE: Mutex<Option<Title>> = Mutex::new(None);

pub fn set_title(title: Title) {
    *TITLE.lock().unwrap_or_else(|e| e.into_inner()) = Some(title);
}

/// The running title; the default (id 0, no storage) until set.
pub fn title() -> Title {
    TITLE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// Gives the process the console's millisecond timers. Titles pace
/// themselves with short waits (the title's audio mixer sleeps
/// about 4 ms per 5.3 ms block) and cannot tune the timer on the console;
/// a desktop process waits in 15.6 ms ticks unless it asks for finer ones,
/// and Windows 11 drops that request while the window is hidden unless the
/// process opts out. Process-wide, for the whole run.
#[cfg(windows)]
pub fn use_console_timers() {
    use windows_sys::Win32::Media::timeBeginPeriod;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling, SetProcessInformation,
    };
    // Also: the system does not run this process at reduced speed (its
    // "efficiency mode": lower clock, the slow cores of a hybrid processor)
    // when its window is not the one in front, as while its owner works in
    // another window or streams. The console has eight cores alike and no
    // such mode. `VICTORY_EFFICIENCY_MODE=1` leaves the system its choice.
    let execution = if std::env::var_os("VICTORY_EFFICIENCY_MODE").is_some_and(|v| v == "1") {
        0
    } else {
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED
    };
    let keep = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION | execution,
        StateMask: 0,
    };
    // SAFETY: plain process settings; `keep` outlives the call.
    unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&raw const keep).cast(),
            size_of_val(&keep) as u32,
        );
        timeBeginPeriod(1);
    }
    ahead_of_other_programs();
    keep_resident();
    trim_driver_cache();
}

/// A minimum working set, off unless asked (Kitty6; foxhound has none). With
/// RAM short (22 GB private on a 32 GB PC, our old base) the system took pages
/// from the game as readily as from idle programs, more so while its window
/// was not in front, and each stall of 0.6-1.3 s lined up with tens to
/// hundreds of thousands of faults.
///
/// - `K6_MIN_WORKING_SET_MB=<MB>`: a soft minimum of that size (a hint: the
///   trimmer goes to processes above their minimum first).
/// - `K6_HARD_WORKING_SET=1`: a hard minimum, by default all of physical
///   memory but 10 GB (never less than half), or `K6_MIN_WORKING_SET_MB`. A
///   page-fault trace on the old base (xperf, 12 s in the world, 2026-11-27)
///   found 408,000 faults in victory.exe, two thirds of them transition faults
///   (pages Windows had trimmed to its standby list). Pages kept this way are
///   not given back while the game runs, so it is for A/B runs.
///
/// If the system refuses the size, it is lowered in steps of 2 GB (a quarter
/// at most) until it takes one or gives up below 1 GB. The right to raise the
/// minimum is every user's (SeIncreaseWorkingSetPrivilege).
#[cfg(windows)]
fn keep_resident() {
    use windows_sys::Win32::System::Memory::{
        QUOTA_LIMITS_HARDWS_MAX_DISABLE, QUOTA_LIMITS_HARDWS_MIN_DISABLE, QUOTA_LIMITS_HARDWS_MIN_ENABLE,
        SetProcessWorkingSetSizeEx,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let hard = std::env::var_os("K6_HARD_WORKING_SET").is_some_and(|v| v == "1");
    let asked_mb = std::env::var("K6_MIN_WORKING_SET_MB").ok().and_then(|v| v.trim().parse::<u64>().ok());
    if !hard && asked_mb.is_none() {
        return;
    }
    // SAFETY: plain data the call fills in.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a sized structure.
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return;
    }
    let Some(mut want) = min_working_set(m.ullTotalPhys, hard, asked_mb) else { return };
    let max = m.ullTotalPhys;
    let flags = if hard { QUOTA_LIMITS_HARDWS_MIN_ENABLE } else { QUOTA_LIMITS_HARDWS_MIN_DISABLE };
    while want >= 1 << 30 {
        // SAFETY: a plain process setting.
        let ok = unsafe {
            SetProcessWorkingSetSizeEx(GetCurrentProcess(), want as usize, max as usize, flags | QUOTA_LIMITS_HARDWS_MAX_DISABLE)
        } != 0;
        if ok {
            tracing::warn!(min_mb = want >> 20, hard, "a minimum working set (K6_MIN_WORKING_SET_MB, K6_HARD_WORKING_SET)");
            return;
        }
        want = want.saturating_sub(2 << 30).min(want / 4 * 3);
    }
    tracing::warn!("the system refused a minimum working set of 1 GB or more");
}

/// The minimum working set asked for, in bytes, on a PC of `total` bytes of
/// RAM; `None` for none.
fn min_working_set(total: u64, hard: bool, asked_mb: Option<u64>) -> Option<u64> {
    match asked_mb {
        Some(0) => None,
        Some(mb) => Some(mb << 20),
        None if hard => Some(total.saturating_sub(10 << 30).max(total / 2)),
        None => None,
    }
}

/// Lets go of the resident pages of the graphics driver's own shader cache files
/// (`K6_TRIM_DRIVER_CACHE_S=<seconds>`: how often; unset or `0`: never). A per-file
/// walk of the game's mapped memory (2026-11-27, in the world) found NVIDIA's
/// `DXCache/*.nvph` mapped at 5 GB with 3.7 GB of it in RAM: driver binaries
/// read when a pipeline is made, then idle, and kept in our working set by the
/// hard minimum (`keep_resident`) where Windows could not take them back. Those
/// pages are clean file pages: `VirtualUnlock` on a range that is not locked
/// takes them out of the working set (documented), they stay in Windows' file
/// cache, and a later read is a soft fault, not a disk read. The views are found
/// by walking the address space (it holds the process's memory map, so rarely:
/// at 20 s, 60 s, then every 5 minutes) and trimmed every period.
#[cfg(windows)]
fn trim_driver_cache() {
    let period = match std::env::var("K6_TRIM_DRIVER_CACHE_S").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(0) | None => return,
        Some(s) => std::time::Duration::from_secs(s),
    };
    let _ = std::thread::Builder::new().name("k6-trim-cache".into()).spawn(move || {
        let began = std::time::Instant::now();
        let mut views: Vec<(usize, usize)> = Vec::new();
        let mut next_walk = std::time::Duration::from_secs(20);
        loop {
            std::thread::sleep(period);
            if began.elapsed() >= next_walk {
                views = driver_cache_views();
                next_walk = began.elapsed()
                    + if next_walk < std::time::Duration::from_secs(60) {
                        std::time::Duration::from_secs(40)
                    } else {
                        std::time::Duration::from_secs(300)
                    };
                let mb: usize = views.iter().map(|v| v.1).sum::<usize>() >> 20;
                tracing::warn!(views = views.len(), mb, "K6_TRIM_DRIVER_CACHE_S: driver shader cache views found");
            }
            for &(at, len) in &views {
                // SAFETY: a range of a view the walk found mapped; on pages that
                // are not locked the call only takes them out of the working set.
                // A view unmapped since fails the call, which changes nothing.
                unsafe { windows_sys::Win32::System::Memory::VirtualUnlock(at as *const _, len) };
            }
        }
    });
}

/// The mapped views of the graphics driver's shader cache files (NVIDIA's
/// `DXCache/*.nvph`, AMD's `DxCache`/`DxcCache`): `(address, length)`.
#[cfg(windows)]
fn driver_cache_views() -> Vec<(usize, usize)> {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_MAPPED, MEMORY_BASIC_INFORMATION, VirtualQuery};
    use windows_sys::Win32::System::ProcessStatus::GetMappedFileNameW;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut name = [0u16; 512];
    loop {
        // SAFETY: plain data the call fills in.
        let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a query of our own address space.
        if unsafe { VirtualQuery(at as *const _, &mut m, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        let base = m.BaseAddress as usize;
        if m.State == MEM_COMMIT && m.Type == MEM_MAPPED && m.RegionSize >= 16 << 20 {
            // SAFETY: a buffer of the length given.
            let n = unsafe { GetMappedFileNameW(GetCurrentProcess(), m.BaseAddress, name.as_mut_ptr(), name.len() as u32) };
            let file = String::from_utf16_lossy(&name[..n as usize]).to_ascii_lowercase();
            if file.ends_with(".nvph") || file.contains("\\dxcache\\") || file.contains("\\dxccache\\") {
                out.push((base, m.RegionSize));
            }
        }
        match base.checked_add(m.RegionSize) {
            Some(next) if next > at => at = next,
            _ => break,
        }
    }
    out
}

/// The title's streaming thread runs below normal priority (on the console
/// nobody else wants its core) and does all of the title's loading: a
/// program that keeps the processors busy at normal priority (a build, a
/// virus scan) starved it, loading took 40 to 60 s instead of 28 to 33, and
/// the opening ran without the models that were to be there (runs cpu-1,
/// play-1125-0203). The process runs above normal, as long as it runs.
/// `VICTORY_PROCESS_PRIORITY=normal`: as it was started.
#[cfg(windows)]
fn ahead_of_other_programs() {
    use windows_sys::Win32::System::Threading::{ABOVE_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass};
    if std::env::var_os("VICTORY_PROCESS_PRIORITY").is_some_and(|v| v == "normal" || v == "0") {
        return;
    }
    // SAFETY: a plain process setting.
    let set = unsafe { SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS) } != 0;
    tracing::info!(set, "the process runs above normal priority (VICTORY_PROCESS_PRIORITY=normal: not)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_minimum_working_set_is_off_unless_asked() {
        const GB: u64 = 1 << 30;
        assert_eq!(min_working_set(32 * GB, false, None), None);
        assert_eq!(min_working_set(32 * GB, true, None), Some(22 * GB));
        assert_eq!(min_working_set(16 * GB, true, None), Some(8 * GB));
        assert_eq!(min_working_set(32 * GB, false, Some(4096)), Some(4 * GB));
        assert_eq!(min_working_set(32 * GB, true, Some(0)), None);
    }

    #[test]
    fn a_quoted_angle_bracket_does_not_end_its_tag() {
        let text = "<Game><ShellVisuals DefaultDisplayName=\"A > B\" Description='x>y'/>\
                    <TitleId>639AA8DD</TitleId>\
                    <PersistentLocalStorage><SizeMB>64</SizeMB></PersistentLocalStorage></Game>";
        let t = Title::from_manifest(text);
        assert_eq!((t.id, t.storage_bytes), (0x639A_A8DD, 64 << 20));
    }

    #[test]
    fn reads_title_id_store_id_and_storage_quota() {
        let text = "\u{feff}<?xml version=\"1.0\"?>\n<Game configVersion=\"1\">\
                    <!-- <TitleId>00000000</TitleId> -->\
                    <Identity Name=\"x\"/>\n  <TitleId>639AA8DD</TitleId>\n\
                    <![CDATA[ <TitleId>1</TitleId> ]]>                    <StoreId>9&amp;C</StoreId><Other><SizeMB>7</SizeMB></Other>\
                    <PersistentLocalStorage>\n  <SizeMB>1024</SizeMB> <!-- c -->\n\
                    </PersistentLocalStorage></Game>";
        let t = Title::from_manifest(text);
        assert_eq!((t.id, t.storage_bytes), (0x639A_A8DD, 1024 << 20));
        assert_eq!(t.store_id, "9&C");
        assert_eq!(Title::from_manifest("").storage_bytes, 0);
    }

    #[test]
    fn inline_comments_do_not_truncate_a_field() {
        let text = "<Game>\
            <TitleId><!-- generated -->639AA8DD</TitleId>\
            <StoreId>9&amp;<!-- keep -->C</StoreId>\
            <PersistentLocalStorage><SizeMB><!-- n -->1024</SizeMB></PersistentLocalStorage>\
            </Game>";
        let t = Title::from_manifest(text);
        assert_eq!(t.id, 0x639A_A8DD);
        assert_eq!(t.store_id, "9&C");
        assert_eq!(t.storage_bytes, 1024 << 20);
    }
}
