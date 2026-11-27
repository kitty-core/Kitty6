//! Diagnostics: probes of a whole run, for telling what a slow PC is short of.
//! `VICTORY_PROBE=<folder>` (outside the source tree) writes
//! `probe-<unix seconds>.csv` there, a row a second, and logs a few lines:
//!
//! - frames: how many, and the spread of their times (mean, median, 95th
//!   percentile, slowest, how many missed 33 / 100 / 500 ms);
//! - the slowest frame of each second that took `VICTORY_PROBE_STALL_MS`
//!   (100) or more, with what happened during it: the process's page
//!   faults, pipelines made, pipelines still waiting (a log line, and three
//!   columns);
//! - the frame line's own numbers: ms preparing, ms on the GPU, draws,
//!   draws dropped without a host pipeline;
//! - host pipelines: waiting, made this second, their mean and longest
//!   making time, the time first uses waited for one;
//! - the process's memory: working set, private bytes, page faults a
//!   second (soft and hard), bytes it read (files and streaming, not paging);
//!   the system's: pages read in from disk a second
//!   (`\Memory\Pages Input/sec`, the hard faults of every process: paging),
//!   free RAM, free commit, memory load;
//! - the card's memory: the system's budget for us and our use of it, and
//!   our use of the PC's memory shared with the card;
//! - processor: the cores the process kept busy, and every 5 s the busiest
//!   threads by name (a log line; the busiest one also in two columns: a
//!   thread near 1.0 cores is a serial wall).
//!
//! `victory probe-report <csv> [<csv>]` reads the file(s) back.
//!
//! The same sampling feeds the Kitty6 overlay's live block (`telemetry`)
//! while the overlay is up and asks for it (its F6), with or without the
//! file. Neither asking, nothing here runs but a check; a build without
//! diagnostics has none of it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS, GetThreadDescription, GetThreadTimes,
    OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
};

/// The columns of the file, in order (`victory probe-report` reads them by name).
pub const COLUMNS: &[&str] = &[
    "t_s",
    "frames",
    "fps",
    "ms_mean",
    "ms_p50",
    "ms_p95",
    "ms_max",
    "over33",
    "over100",
    "over500",
    "stall_ms",
    "stall_faults",
    "stall_pipelines_made",
    "prep_ms",
    "gpu_ms",
    "draws",
    "dropped",
    "pipelines_waiting",
    "pipelines_made",
    "pipeline_ms_mean",
    "pipeline_ms_max",
    "pipeline_wait_ms",
    "ws_mb",
    "private_mb",
    "faults_per_s",
    "io_read_mb_s",
    "sys_pages_in_per_s",
    "avail_ram_mb",
    "avail_commit_mb",
    "mem_load_pct",
    "vram_budget_mb",
    "vram_used_mb",
    "vram_shared_mb",
    "cpu_cores",
    "busiest_thread",
    "busiest_thread_cores",
    "focused",
];

/// Whether the window in front is one of this process's: Windows gives the
/// window in front longer time slices, the GPU scheduler's favour and the
/// last say on memory, and runs differ by several frames a second with it.
#[cfg(windows)]
fn focused() -> bool {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: plain queries; a null window gives process 0.
    unsafe {
        let window = GetForegroundWindow();
        let mut pid = 0u32;
        !window.is_null() && GetWindowThreadProcessId(window, &mut pid) != 0 && pid == GetCurrentProcessId()
    }
}

/// The folder asked for, made and allowed; `None`: the probes are off.
fn folder() -> Option<&'static PathBuf> {
    static FOLDER: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOLDER
        .get_or_init(|| {
            let path = PathBuf::from(std::env::var_os("VICTORY_PROBE")?);
            if let Err(e) = victory_core::policy::ensure_outside_repo(&path) {
                tracing::warn!("VICTORY_PROBE refused: {e}");
                return None;
            }
            if let Err(e) = std::fs::create_dir_all(&path) {
                tracing::warn!(path = %path.display(), "VICTORY_PROBE: the folder could not be made: {e}");
                return None;
            }
            Some(path)
        })
        .as_ref()
}

/// Whether the probes' file is asked for.
fn file_on() -> bool {
    folder().is_some()
}

/// Whether the probes run: for the file, or for the overlay while it asks.
pub(crate) fn on() -> bool {
    file_on() || crate::telemetry::tracking()
}

/// Host pipelines the host refused to make, since the start (counted
/// whether the probes run or not: it happens a handful of times a run).
static REFUSED: AtomicU64 = AtomicU64::new(0);

/// The host refused a pipeline.
pub(crate) fn pipeline_refused() {
    REFUSED.fetch_add(1, Relaxed);
}

/// The swap chain's size, width in the high half.
static SIZE: AtomicU64 = AtomicU64::new(0);

/// The swap chain's size as presented (two loads and a store at most).
pub(crate) fn set_resolution(width: u32, height: u32) {
    let packed = (u64::from(width) << 32) | u64::from(height);
    if SIZE.load(Relaxed) != packed {
        SIZE.store(packed, Relaxed);
    }
}

/// The swap chain's size.
pub(crate) fn resolution() -> (u32, u32) {
    let packed = SIZE.load(Relaxed);
    ((packed >> 32) as u32, packed as u32)
}

/// A frame this long or longer is a stall (`VICTORY_PROBE_STALL_MS`, 100).
fn stall_ms() -> f32 {
    static MS: OnceLock<f32> = OnceLock::new();
    *MS.get_or_init(|| {
        std::env::var("VICTORY_PROBE_STALL_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .filter(|ms: &f32| *ms > 0.0)
            .unwrap_or(100.0)
    })
}

// Host pipelines, since the last row (and made in all, for the stalls).
static MADE: AtomicU64 = AtomicU64::new(0);
static MADE_ALL: AtomicU64 = AtomicU64::new(0);
static MAKING_US: AtomicU64 = AtomicU64::new(0);
static MAKING_MAX_US: AtomicU64 = AtomicU64::new(0);
static WAITED_US: AtomicU64 = AtomicU64::new(0);

/// A host pipeline was made in `took`.
pub(crate) fn pipeline_made(took: Duration) {
    if !on() {
        return;
    }
    let us = took.as_micros() as u64;
    MADE.fetch_add(1, Relaxed);
    MADE_ALL.fetch_add(1, Relaxed);
    MAKING_US.fetch_add(us, Relaxed);
    MAKING_MAX_US.fetch_max(us, Relaxed);
}

/// A first use of a pipeline waited `waited` for its host pipeline.
pub(crate) fn pipeline_waited(waited: Duration) {
    if on() {
        WAITED_US.fetch_add(waited.as_micros() as u64, Relaxed);
    }
}

/// The frames of the second under way.
#[derive(Default)]
struct Frames {
    last: Option<Instant>,
    last_faults: u32,
    last_made: u64,
    times: Vec<f32>,
    /// The slowest frame at or over the stall time: ms, page faults, pipelines made.
    stall: Option<(f32, u32, u64)>,
}

static FRAMES: Mutex<Option<Frames>> = Mutex::new(None);

/// The process's page faults so far (soft and hard).
fn page_faults() -> u32 {
    process_memory().map_or(0, |c| c.PageFaultCount)
}

fn process_memory() -> Option<PROCESS_MEMORY_COUNTERS_EX> {
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: our own process; the extended structure starts with the plain one
    // and `cb` says which it is.
    let ok = unsafe {
        K32GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb)
    };
    (ok != 0).then_some(c)
}

/// The probes ran at the last present (off and on again, the first frame
/// after is not measured from a present long ago).
static RAN: AtomicBool = AtomicBool::new(false);

/// A frame was presented to `hwnd` (from the presenting thread: a clock
/// read, a memory query, a lock nobody else holds for long; with neither
/// the file nor the overlay asking, two checks).
pub(crate) fn frame(hwnd: usize) {
    crate::telemetry::window(hwnd);
    let live = crate::telemetry::tracking();
    if !live && !file_on() {
        RAN.store(false, Relaxed);
        return;
    }
    let restart = !RAN.swap(true, Relaxed);
    let now = Instant::now();
    let faults = page_faults();
    let made = MADE_ALL.load(Relaxed);
    let measured = {
        let mut held = FRAMES.lock().unwrap_or_else(|e| e.into_inner());
        let f = held.get_or_insert_with(Frames::default);
        if restart {
            f.last = None;
            f.times.clear();
            f.stall = None;
        }
        let mut measured = None;
        if let Some(last) = f.last {
            let ms = now.duration_since(last).as_secs_f32() * 1000.0;
            let frame_faults = faults.wrapping_sub(f.last_faults);
            f.times.push(ms);
            if ms >= stall_ms() && f.stall.is_none_or(|(worst, _, _)| ms > worst) {
                f.stall = Some((ms, frame_faults, made - f.last_made));
            }
            measured = Some((ms, frame_faults));
        }
        f.last = Some(now);
        f.last_faults = faults;
        f.last_made = made;
        measured
    };
    if let (true, Some((ms, frame_faults))) = (live, measured) {
        crate::telemetry::frame(ms, frame_faults);
    }
}

/// The spread of a second's frame times.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spread {
    pub mean: f32,
    pub p50: f32,
    pub p95: f32,
    pub max: f32,
    pub over33: u32,
    pub over100: u32,
    pub over500: u32,
}

/// The value at fraction `q` (0 to 1) of sorted `v` (nearest rank).
pub fn quantile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (q * sorted.len() as f32).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// The spread of frame times `ms` (any order).
pub fn spread(ms: &mut [f32]) -> Spread {
    if ms.is_empty() {
        return Spread::default();
    }
    ms.sort_by(f32::total_cmp);
    let over = |limit: f32| ms.iter().filter(|&&t| t > limit).count() as u32;
    Spread {
        mean: ms.iter().sum::<f32>() / ms.len() as f32,
        p50: quantile(ms, 0.5),
        p95: quantile(ms, 0.95),
        max: ms[ms.len() - 1],
        over33: over(33.4),
        over100: over(100.0),
        over500: over(500.0),
    }
}

/// What the frame line measured over the second (from the meter's thread).
pub(crate) struct Line {
    pub(crate) frames: f64,
    pub(crate) fps: f64,
    pub(crate) prep_ms: f64,
    pub(crate) gpu_ms: f64,
    pub(crate) draws: f64,
    pub(crate) dropped: f64,
    pub(crate) waiting: u64,
}

/// The system's paging counter.
struct Pdh {
    query: PDH_HQUERY,
    pages_in: PDH_HCOUNTER,
}

// SAFETY: the handles are used by the meter's thread only, under `OUT`.
unsafe impl Send for Pdh {}

impl Pdh {
    fn open() -> Option<Pdh> {
        let mut query: PDH_HQUERY = std::ptr::null_mut();
        let mut pages_in: PDH_HCOUNTER = std::ptr::null_mut();
        let path: Vec<u16> = "\\Memory\\Pages Input/sec\0".encode_utf16().collect();
        // SAFETY: out pointers to our handles; a terminated wide string.
        unsafe {
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut pages_in) != 0 {
                return None;
            }
            PdhCollectQueryData(query);
        }
        Some(Pdh { query, pages_in })
    }

    /// Pages read in a second since the last call.
    fn pages_in(&self) -> f64 {
        let mut value = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: our query and counter; a value structure of the asked kind.
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return f64::NAN;
            }
            if PdhGetFormattedCounterValue(self.pages_in, PDH_FMT_DOUBLE, std::ptr::null_mut(), &mut value) != 0 {
                return f64::NAN;
            }
            value.Anonymous.doubleValue
        }
    }
}

fn filetime(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// The processor time of the process so far, in 100 ns units.
fn process_cpu() -> u64 {
    let mut times = [FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }; 4];
    let [created, exited, kernel, user] = &mut times;
    // SAFETY: our own process and four structures to fill.
    if unsafe { GetProcessTimes(GetCurrentProcess(), created, exited, kernel, user) } == 0 {
        return 0;
    }
    filetime(times[2]) + filetime(times[3])
}

/// Bytes the process has read so far (files; paging is not counted).
fn process_read() -> u64 {
    let mut io: IO_COUNTERS = unsafe { std::mem::zeroed() };
    // SAFETY: our own process and a structure to fill.
    if unsafe { GetProcessIoCounters(GetCurrentProcess(), &mut io) } == 0 {
        return 0;
    }
    io.ReadTransferCount
}

/// Every thread of the process: id, name and processor time (100 ns units).
fn threads() -> Vec<(u32, String, u64)> {
    let mut out = Vec::new();
    // SAFETY: a snapshot we close below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return out;
    }
    // SAFETY: plain query.
    let pid = unsafe { GetCurrentProcessId() };
    let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..unsafe { std::mem::zeroed() } };
    // SAFETY: the snapshot and a sized entry.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            let tid = entry.th32ThreadID;
            // SAFETY: a query-only handle we close.
            let thread = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid) };
            if !thread.is_null() {
                let mut times = [FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }; 4];
                let [created, exited, kernel, user] = &mut times;
                // SAFETY: the thread's handle and four structures to fill.
                let cpu = if unsafe { GetThreadTimes(thread, created, exited, kernel, user) } != 0 {
                    filetime(times[2]) + filetime(times[3])
                } else {
                    0
                };
                let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
                let mut name = String::new();
                // SAFETY: the system allocates the text; we free it with LocalFree.
                if unsafe { GetThreadDescription(thread, &mut text) } >= 0 && !text.is_null() {
                    // SAFETY: a terminated wide string from the system.
                    let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
                    // SAFETY: `len` characters lie there.
                    name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
                    // SAFETY: allocated by the system for us.
                    unsafe { LocalFree(text.cast()) };
                }
                // SAFETY: our handle.
                unsafe { CloseHandle(thread) };
                out.push((tid, name, cpu));
            }
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: our snapshot.
    unsafe { CloseHandle(snapshot) };
    out
}

/// A thread's name without its number, so that a pool's threads add up:
/// `victory-thin-3` and `victory-thin-12` are `victory-thin-#`.
pub fn thread_group(name: &str) -> String {
    if name.is_empty() {
        return "(unnamed)".into();
    }
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.len() == name.len() { name.to_owned() } else { format!("{trimmed}#") }
}

/// What the next row is measured against, and the probes' file (when asked).
struct Out {
    file: Option<BufWriter<File>>,
    /// The file could not be made or written: not tried again.
    file_failed: bool,
    started: Instant,
    wall: Instant,
    cpu: u64,
    faults: u32,
    read: u64,
    pdh: Option<Pdh>,
    threads: HashMap<u32, u64>,
    threads_at: Instant,
    busiest: (String, f64),
    /// The busiest threads and kinds of thread at the last look, for the overlay.
    top: Vec<(String, f64)>,
    kinds: Vec<(String, f64)>,
    /// Texture uploads counted at the last row (the counter goes back to 0
    /// with the 5 s copy line).
    uploads: u64,
}

static OUT: Mutex<Option<Out>> = Mutex::new(None);

impl Out {
    fn new() -> Out {
        let now = Instant::now();
        Out {
            file: None,
            file_failed: false,
            started: now,
            wall: now,
            cpu: process_cpu(),
            faults: page_faults(),
            read: process_read(),
            pdh: Pdh::open(),
            threads: threads().into_iter().map(|(tid, _, cpu)| (tid, cpu)).collect(),
            threads_at: now,
            busiest: (String::new(), 0.0),
            top: Vec::new(),
            kinds: Vec::new(),
            uploads: crate::copystat::peek(crate::copystat::C::TexUploads),
        }
    }
}

fn open_file() -> Option<BufWriter<File>> {
    let folder = folder()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = folder.join(format!("probe-{stamp}.csv"));
    let mut file = match File::create(&path) {
        Ok(f) => BufWriter::new(f),
        Err(e) => {
            tracing::warn!(path = %path.display(), "VICTORY_PROBE: the file could not be made: {e}");
            return None;
        }
    };
    writeln!(file, "{}", COLUMNS.join(",")).ok()?;
    tracing::info!(path = %path.display(), stall_ms = stall_ms(), "probes on (VICTORY_PROBE): a row a second");
    Some(file)
}

/// Every 30 s (2 s for the overlay alone): the threads' processor time since
/// the last look, the busiest kept for the rows and the overlay, the
/// busiest ten logged when the file is on.
fn look_at_threads(out: &mut Out, now: Instant, file: bool) {
    let wall = now.duration_since(out.threads_at).as_secs_f64();
    // A thread snapshot maps a system-wide list (~32 MB of fresh pages each
    // time: 21,600 demand-zero faults in 12 s in a fault trace), so the file
    // looks every 30 s (`K6_PROBE_THREADS_S`), the overlay every 2 s.
    static EVERY: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    let every = *EVERY.get_or_init(|| std::env::var("K6_PROBE_THREADS_S").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(30.0));
    if wall < if file { every } else { 2.0 } {
        return;
    }
    out.threads_at = now;
    let all = threads();
    let mut busy: Vec<(f64, String, u32)> = Vec::with_capacity(all.len());
    let mut groups: HashMap<String, f64> = HashMap::new();
    let mut seen = HashMap::with_capacity(all.len());
    for (tid, name, cpu) in all {
        let before = out.threads.get(&tid).copied().unwrap_or(cpu);
        let cores = cpu.saturating_sub(before) as f64 / 1e7 / wall;
        seen.insert(tid, cpu);
        *groups.entry(thread_group(&name)).or_default() += cores;
        busy.push((cores, name, tid));
    }
    out.threads = seen;
    busy.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut groups: Vec<(String, f64)> = groups.into_iter().collect();
    groups.sort_by(|a, b| b.1.total_cmp(&a.1));
    let label = |name: &String, tid: u32| if name.is_empty() { format!("tid{tid}") } else { name.clone() };
    out.busiest = busy.first().map(|(cores, name, tid)| (label(name, *tid), *cores)).unwrap_or_default();
    out.top = busy.iter().take(victory_core::telemetry::THREADS).map(|(c, n, t)| (label(n, *t), *c)).collect();
    out.kinds = groups.iter().take(victory_core::telemetry::THREADS).cloned().collect();
    if !file {
        return;
    }
    let top: Vec<String> = busy
        .iter()
        .take(10)
        .map(|(cores, name, tid)| format!("{}[{tid}] {cores:.2}", if name.is_empty() { "(unnamed)" } else { name }))
        .collect();
    let by_group: Vec<String> = groups.iter().take(8).map(|(g, c)| format!("{g} {c:.2}")).collect();
    tracing::info!(
        target: "victory_gpu::probe",
        "busiest threads over {wall:.0} s, in cores: {}; by kind: {}",
        top.join(", "),
        by_group.join(", ")
    );
}

/// A row: the second's frames and what the frame line measured, with the
/// process's, the system's and the card's state (from the meter's thread),
/// written to the file and, while the overlay asks, to its block.
pub(crate) fn second(line: Line) {
    let file = file_on();
    let live = crate::telemetry::tracking();
    if !file && !live {
        return;
    }
    let (mut times, stall) = {
        let mut held = FRAMES.lock().unwrap_or_else(|e| e.into_inner());
        let f = held.get_or_insert_with(Frames::default);
        (std::mem::take(&mut f.times), f.stall.take())
    };
    let s = spread(&mut times);
    let mut held = OUT.lock().unwrap_or_else(|e| e.into_inner());
    let out = held.get_or_insert_with(Out::new);
    if file && out.file.is_none() && !out.file_failed {
        out.file = open_file();
        out.file_failed = out.file.is_none();
    }
    let now = Instant::now();
    let wall = now.duration_since(out.wall).as_secs_f64().max(1e-3);
    out.wall = now;

    let memory = process_memory().unwrap_or_default();
    let faults = memory.PageFaultCount.wrapping_sub(out.faults) as f64 / wall;
    out.faults = memory.PageFaultCount;
    let read = process_read();
    let read_mb_s = read.saturating_sub(out.read) as f64 / 1048576.0 / wall;
    out.read = read;
    let cpu = process_cpu();
    let cores = cpu.saturating_sub(out.cpu) as f64 / 1e7 / wall;
    out.cpu = cpu;
    let mut system: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    system.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a sized structure to fill.
    unsafe { GlobalMemoryStatusEx(&mut system) };
    let pages_in = out.pdh.as_ref().map_or(f64::NAN, Pdh::pages_in);
    let video = crate::host::host().and_then(|h| h.video_memory()).unwrap_or_default();
    look_at_threads(out, now, out.file.is_some());

    let made = MADE.swap(0, Relaxed);
    let making_us = MAKING_US.swap(0, Relaxed);
    let making_max = MAKING_MAX_US.swap(0, Relaxed);
    let waited = WAITED_US.swap(0, Relaxed);
    let mb = |b: u64| b >> 20;
    let (stall_ms, stall_faults, stall_made) = stall.unwrap_or((0.0, 0, 0));
    if stall.is_some() && out.file.is_some() {
        tracing::info!(
            target: "victory_gpu::probe",
            "slowest frame of the second: {stall_ms:.0} ms, {stall_faults} page faults and {stall_made} host pipelines made during it, {} pipelines waiting; this second: {:.0} pages read in from disk by the system, {} MB RAM free, {} of {} MB of the card's memory in use",
            line.waiting,
            pages_in,
            mb(system.ullAvailPhys),
            mb(video[1]),
            mb(video[0])
        );
    }
    let t_s = out.started.elapsed().as_secs_f64();
    let pipeline_ms_mean = if made > 0 { making_us as f64 / made as f64 / 1e3 } else { 0.0 };
    if let Some(writer) = out.file.as_mut() {
        let row = format!(
            "{:.1},{:.0},{:.2},{:.1},{:.1},{:.1},{:.1},{},{},{},{:.1},{},{},{:.1},{:.1},{:.0},{:.0},{},{},{:.1},{:.1},{:.1},{},{},{:.0},{:.1},{:.0},{},{},{},{},{},{},{:.2},{},{:.2},{}",
            t_s,
            line.frames,
            line.fps,
            s.mean,
            s.p50,
            s.p95,
            s.max,
            s.over33,
            s.over100,
            s.over500,
            stall_ms,
            stall_faults,
            stall_made,
            line.prep_ms,
            line.gpu_ms,
            line.draws,
            line.dropped,
            line.waiting,
            made,
            pipeline_ms_mean,
            making_max as f64 / 1e3,
            waited as f64 / 1e3,
            mb(memory.WorkingSetSize as u64),
            mb(memory.PrivateUsage as u64),
            faults,
            read_mb_s,
            pages_in,
            mb(system.ullAvailPhys),
            mb(system.ullAvailPageFile),
            system.dwMemoryLoad,
            mb(video[0]),
            mb(video[1]),
            mb(video[3]),
            cores,
            out.busiest.0.replace(',', ";"),
            out.busiest.1,
            u8::from(focused()),
        );
        debug_assert_eq!(row.split(',').count(), COLUMNS.len(), "a probe row and its columns differ");
        if writeln!(writer, "{row}").and_then(|()| writer.flush()).is_err() {
            tracing::warn!("VICTORY_PROBE: a row could not be written; the probes' file stops");
            out.file = None;
            out.file_failed = true;
        }
    }
    if !live {
        return;
    }
    // The overlay's extras: what holds memory, and counters since the start.
    use std::sync::atomic::Ordering::Relaxed as R;
    let mbf = |b: u64| (b as f64 / 1048576.0) as f32;
    let copies = crate::shadow::live();
    let textures = [0, 1].map(|i| crate::resource::HOST_TEXTURES[i].load(R));
    let staged = crate::texture::memory();
    let alive = [2, 5, 7].map(|i| crate::host::ALIVE[i].load(R));
    let uploads = crate::copystat::peek(crate::copystat::C::TexUploads);
    let uploaded = if uploads >= out.uploads { uploads - out.uploads } else { uploads };
    out.uploads = uploads;
    let second = victory_core::telemetry::Second {
        t_s: t_s as f32,
        frames: line.frames as f32,
        fps: line.fps as f32,
        ms_mean: s.mean,
        ms_p50: s.p50,
        ms_p95: s.p95,
        ms_max: s.max,
        over33: s.over33 as f32,
        over100: s.over100 as f32,
        over500: s.over500 as f32,
        prep_ms: line.prep_ms as f32,
        gpu_ms: line.gpu_ms as f32,
        draws: line.draws as f32,
        dropped: line.dropped as f32,
        pipelines_waiting: line.waiting as f32,
        pipelines_made: made as f32,
        pipeline_ms_mean: pipeline_ms_mean as f32,
        pipeline_ms_max: (making_max as f64 / 1e3) as f32,
        pipeline_wait_ms: (waited as f64 / 1e3) as f32,
        pipelines_refused: REFUSED.load(R) as f32,
        ws_mb: mbf(memory.WorkingSetSize as u64),
        private_mb: mbf(memory.PrivateUsage as u64),
        faults_per_s: faults as f32,
        io_read_mb_s: read_mb_s as f32,
        sys_pages_in_per_s: pages_in as f32,
        avail_ram_mb: mbf(system.ullAvailPhys),
        total_ram_mb: mbf(system.ullTotalPhys),
        avail_commit_mb: mbf(system.ullAvailPageFile),
        commit_limit_mb: mbf(system.ullTotalPageFile),
        mem_load_pct: system.dwMemoryLoad as f32,
        vram_budget_mb: mbf(video[0]),
        vram_used_mb: mbf(video[1]),
        vram_shared_mb: mbf(video[3]),
        cpu_cores: cores as f32,
        busiest_thread_cores: out.busiest.1 as f32,
        host_copies: copies[0] as f32,
        host_copies_mb: mbf(copies[1]),
        host_textures: textures[0] as f32,
        host_textures_mb: mbf(textures[1]),
        staging_mb: mbf(staged[1]),
        upload_buffers: alive[1] as f32,
        upload_mb: mbf(alive[2]),
        scratch_mb: mbf(alive[0]),
        texture_uploads_per_s: (uploaded as f64 / wall) as f32,
        allocators_renewed: crate::host::RENEWED.load(R) as f32,
        left_out_levels: crate::list::LEFT_OUT_LEVELS.load(R) as f32,
    };
    let stall = stall.map(|(ms, faults, made)| victory_core::telemetry::Stall {
        t_s: t_s as f32,
        ms,
        faults: faults as f32,
        pipelines_made: made as f32,
        pipelines_waiting: line.waiting as f32,
        sys_pages_in: pages_in as f32,
    });
    crate::telemetry::second(
        second,
        crate::telemetry::Extra { stall, threads: &out.top, groups: &out.kinds, stall_ms: self::stall_ms() },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_of_frames() {
        let mut ms = vec![40.0, 30.0, 600.0, 33.0, 120.0];
        let s = spread(&mut ms);
        assert_eq!(s.p50, 40.0);
        assert_eq!(s.max, 600.0);
        assert_eq!(s.p95, 600.0);
        assert_eq!((s.over33, s.over100, s.over500), (3, 2, 1));
        assert!((s.mean - 164.6).abs() < 0.01);
        assert_eq!(spread(&mut []), Spread::default());
    }

    #[test]
    fn quantile_ranks() {
        let v: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        assert_eq!(quantile(&v, 0.5), 50.0);
        assert_eq!(quantile(&v, 0.95), 95.0);
        assert_eq!(quantile(&v, 0.0), 1.0);
        assert_eq!(quantile(&v, 1.0), 100.0);
    }

    /// The system queries, on the test process itself.
    #[test]
    fn system_queries_answer() {
        let memory = process_memory().expect("process memory");
        assert!(memory.WorkingSetSize > 0 && memory.PrivateUsage > 0 && memory.PageFaultCount > 0);
        let before = process_cpu();
        // Busy for 100 ms: the system counts processor time in ticks of about 15.6 ms.
        let spin = Instant::now();
        let mut x = 0u64;
        while spin.elapsed() < Duration::from_millis(100) {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(1));
        }
        assert!(process_cpu() > before, "processor time moves");
        let _ = process_read();
        let named = std::thread::Builder::new()
            .name("probe-test-7".into())
            .spawn(|| {
                let all = threads();
                // SAFETY: plain query.
                let me = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
                all.into_iter().find(|t| t.0 == me).map(|t| t.1)
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(named.as_deref(), Some("probe-test-7"), "the standard library names threads for the system");
        let pdh = Pdh::open().expect("the paging counter opens");
        std::thread::sleep(Duration::from_millis(200));
        let pages = pdh.pages_in();
        assert!(pages.is_finite() && pages >= 0.0, "{pages}");
    }

    #[test]
    fn thread_groups() {
        assert_eq!(thread_group("victory-thin-12"), "victory-thin-#");
        assert_eq!(thread_group("victory-meter"), "victory-meter");
        assert_eq!(thread_group(""), "(unnamed)");
    }
}
