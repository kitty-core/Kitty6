//! What the samples say. Works on named stacks only (no Windows calls), so
//! it is tested with made-up stacks.
//!
//! A sample *waits* when the frames at its top that belong to the system
//! (ntdll/kernelbase/..., the card driver, Rust's std/parking_lot sync code)
//! include a blocking call (NtWaitFor*, NtDelayExecution, SleepConditionVariable,
//! RtlAcquireSRWLock*, RtlpWaitOnCriticalSection, WaitOnAddress, park...).
//! The wait is put on its *site*, the first frame above those that is not the
//! system's, and given a kind: lock, event (condition variables, parking,
//! kernel events), fence (the card: a kernel event waited for by the card
//! code, or a wait inside the D3D12 runtime/driver), sleep, idle (thread
//! pool / IO completion), message (window messages), other.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

pub type Map = HashMap<String, u64>;

pub struct ThreadData {
    pub tid: u32,
    /// The thread's description with numbers folded ("victory-list-#"), or
    /// "(unnamed TID)".
    pub group: String,
    pub described: bool,
    /// Stacks as indices into `Profile::names`, leaf first, with counts.
    pub stacks: Vec<(Vec<u32>, u64)>,
}

pub struct Profile {
    pub pid: u32,
    pub seconds: f64,
    pub hz: f64,
    pub rounds: u64,
    pub names: Vec<String>,
    pub threads: Vec<ThreadData>,
    /// The sampler's own CPU time while sampling, s.
    pub sampler_cpu_s: f64,
    /// Wall time the sampler spent sampling (not sleeping), s.
    pub sampler_busy_s: f64,
    /// Time the target's threads spent suspended by the sampler, s (summed).
    pub stall_s: f64,
    pub walks: u64,
    /// Samples of threads that had not run since their last walk (counted
    /// again without suspending them).
    pub reused: u64,
}

impl Profile {
    /// Thread-ms per second of wall time that `count` samples stand for: each
    /// round samples every thread once.
    pub fn ms_per_s(&self, count: u64) -> f64 {
        1000.0 * count as f64 / self.rounds.max(1) as f64
    }
}

pub struct Options {
    pub top: usize,
    pub focus: Option<String>,
    /// Extra lowercase patterns marking a kernel event wait as a card fence.
    pub fence: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options { top: 25, focus: None, fence: Vec::new() }
    }
}

/// "victory-list-3" -> "victory-list-#", "uploads ahead 2" -> "uploads ahead #".
pub fn fold_number(s: &str) -> String {
    let t = s.trim_end_matches(|c: char| c.is_ascii_digit());
    if t.len() == s.len() { s.to_owned() } else { format!("{t}#") }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Kind {
    Lock,
    Event,
    Fence,
    Sleep,
    Idle,
    Message,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Lock, Kind::Event, Kind::Fence, Kind::Sleep, Kind::Idle, Kind::Message, Kind::Other];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Lock => "lock",
            Kind::Event => "event",
            Kind::Fence => "fence",
            Kind::Sleep => "sleep",
            Kind::Idle => "idle",
            Kind::Message => "message",
            Kind::Other => "other",
        }
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s)
    }
}

const SYSTEM_MODULES: &[&str] = &[
    "ntdll", "kernelbase", "kernel32", "win32u", "user32", "ucrtbase", "vcruntime140", "vcruntime140_1", "msvcp140",
    "combase", "rpcrt4", "sechost", "advapi32", "ws2_32", "mswsock", "gdi32", "gdi32full", "msvcrt",
];
/// The D3D runtime and card drivers: a wait inside them is the card's.
const CARD_MODULES: &[&str] = &[
    "d3d12", "d3d12core", "d3d11", "dxgi", "dxcore", "d3d12sdklayers", "nvwgf2umx", "nvldumdx", "nvapi64", "nvoglv64",
    "amdxc64", "amdxx64", "amdihk64", "igd12umd64", "igdumdim64", "igc64",
];
/// Rust runtime and sync crates: callers of these are the site.
const RUNTIME_PREFIXES: &[&str] = &[
    "std::sys::", "std::sync::", "std::thread::park", "std::thread::Thread::park", "std::thread::sleep",
    "std::thread::functions::sleep", "std::sys_common::", "parking_lot::", "parking_lot_core::", "lock_api::",
    "core::sync::", "crossbeam_channel::", "crossbeam_utils::", "std::io::stdio::",
];
/// Calls that block (lowercase, matched on the name after "module!").
const BLOCKING: &[&str] = &[
    "waitforsingleobject", "waitformultipleobjects", "waitforalertbythreadid", "delayexecution", "waitforkeyedevent",
    "removeiocompletion", "waitforworkviaworkerfactory", "msgwaitformultipleobjects", "ntusergetmessage",
    "ntuserwaitmessage", "signalandwaitforsingleobject", "sleepconditionvariable", "acquiresrwlock",
    "rtlpwaitoncriticalsection", "waitonaddress", "sleepex", "getqueuedcompletionstatus", "std::thread::park",
    "std::thread::thread::park", "std::thread::sleep", "parking_lot_core::parking_lot::park", "futex_wait",
    "thread_parking", "lock_contended", "lock_slow",
];
/// Default site patterns for card fence waits on kernel events (victory's
/// fence waits are SetEventOnCompletion + WaitForSingleObject in these).
const FENCE_SITES: &[&str] = &["fence", "seteventoncompletion", "settle", "victory_gpu::host", "victory_gpu::queue"];

/// The module of a frame, for "module!name" and "module.dll+0x..." frames.
pub fn module_of(frame: &str) -> Option<&str> {
    if let Some(i) = frame.find('!') {
        let m = &frame[..i];
        if !m.is_empty() && !m.contains([':', ' ', '<']) {
            return Some(m);
        }
    }
    if let Some(i) = frame.find("+0x") {
        let m = &frame[..i];
        let stem = m.rsplit_once('.').map_or(m, |(s, _)| s);
        if !stem.is_empty() && !stem.contains([':', ' ']) {
            return Some(stem);
        }
    }
    None
}

fn in_modules(frame: &str, list: &[&str]) -> bool {
    module_of(frame).is_some_and(|m| list.iter().any(|x| m.eq_ignore_ascii_case(x)))
}

pub fn is_card(frame: &str) -> bool {
    in_modules(frame, CARD_MODULES)
}

/// Frames that are the system's, not ours (Windows, the card driver, Rust's
/// sync runtime).
pub fn is_system(frame: &str) -> bool {
    in_modules(frame, SYSTEM_MODULES) || is_card(frame) || RUNTIME_PREFIXES.iter().any(|p| frame.starts_with(p))
}

fn bare(frame: &str) -> String {
    frame.rsplit_once('!').map_or(frame, |(m, n)| if m.contains([':', ' ']) { frame } else { n }).to_ascii_lowercase()
}

fn is_thread_start(frame: &str) -> bool {
    let b = bare(frame);
    b == "rtluserthreadstart" || b == "basethreadinitthunk"
}

/// A sample that waits: the index of its site in the frames, the site's
/// name and the kind.
#[derive(Debug, PartialEq)]
pub struct Wait {
    pub site: String,
    pub at: usize,
    pub kind: Kind,
}

/// Whether the stack (leaf first) waits, and where.
pub fn classify(frames: &[&str], fence: &[String]) -> Option<Wait> {
    let sys = frames.iter().take_while(|f| is_system(f)).count();
    if sys == 0 {
        return None;
    }
    let region: Vec<String> = frames[..sys].iter().map(|f| bare(f)).collect();
    let any = |pats: &[&str]| region.iter().any(|f| pats.iter().any(|p| f.contains(p)));
    if !any(BLOCKING) && !region.iter().any(|f| f == "sleep") {
        return None;
    }
    let (site, at) = if sys < frames.len() {
        (frames[sys].to_string(), sys)
    } else {
        // All the system's (a pool worker waiting for work): the outermost
        // frame below the thread start.
        let i = frames.iter().rposition(|f| !is_thread_start(f)).unwrap_or(0);
        (format!("[system] {}", frames[i]), i)
    };
    let kernel_event = any(&["waitforsingleobject", "waitformultipleobjects", "signalandwait"]);
    let kind = if any(&["acquiresrwlock", "criticalsection", "mutex", "rwlock", "lock_contended", "lock_slow", "lock_api"]) {
        Kind::Lock
    } else if any(&["condition", "condvar", "park"]) {
        Kind::Event
    } else if frames[..sys].iter().any(|f| is_card(f)) {
        Kind::Fence
    } else if any(&["delayexecution", "sleepex", "::sleep"]) || region.iter().any(|f| f == "sleep") {
        Kind::Sleep
    } else if any(&["removeiocompletion", "workerfactory", "getqueuedcompletion"]) {
        Kind::Idle
    } else if any(&["msgwait", "getmessage", "waitmessage"]) {
        Kind::Message
    } else if kernel_event {
        // A kernel event: the card's when the site (or a few callers above
        // it) is the card code.
        let above: Vec<String> = frames[at.min(frames.len())..(at + 4).min(frames.len())].iter().map(|f| f.to_ascii_lowercase()).collect();
        let is_fence = above.iter().any(|f| FENCE_SITES.iter().any(|p| f.contains(p)) || fence.iter().any(|p| f.contains(p.as_str())));
        if is_fence { Kind::Fence } else { Kind::Event }
    } else if any(&["waitonaddress", "futex", "alertbythreadid", "keyedevent"]) {
        Kind::Event
    } else {
        Kind::Other
    };
    Some(Wait { site, at, kind })
}

#[derive(Default, Clone)]
pub struct Stats {
    pub samples: u64,
    /// Samples not waiting.
    pub busy: u64,
    pub threads: usize,
    pub wait_kind: [u64; 7],
    /// Leaf of every sample.
    pub self_all: Map,
    /// Leaf of samples not waiting.
    pub self_busy: Map,
    pub inclusive: Map,
    /// First frame of ours: what the thread runs, or "[kind] site" it waits at.
    pub what: Map,
    /// "what <- caller <- caller" chains.
    pub chains: Map,
    /// (site, kind) -> samples.
    pub waits: HashMap<(String, Kind), u64>,
}

impl Stats {
    pub fn waiting(&self) -> u64 {
        self.samples - self.busy
    }

    pub fn add(&mut self, frames: &[&str], n: u64, fence: &[String]) {
        if frames.is_empty() {
            return;
        }
        self.samples += n;
        *self.self_all.entry(frames[0].to_string()).or_default() += n;
        let mut seen = HashSet::new();
        for f in frames {
            if seen.insert(*f) {
                *self.inclusive.entry(f.to_string()).or_default() += n;
            }
        }
        match classify(frames, fence) {
            Some(w) => {
                self.wait_kind[w.kind as usize] += n;
                *self.what.entry(format!("[{}] {}", w.kind.name(), w.site)).or_default() += n;
                let chain: Vec<&str> = frames[w.at..].iter().take(5).copied().collect();
                *self.chains.entry(format!("[{}] {}", w.kind.name(), chain.join("  <-  "))).or_default() += n;
                *self.waits.entry((w.site, w.kind)).or_default() += n;
            }
            None => {
                self.busy += n;
                *self.self_busy.entry(frames[0].to_string()).or_default() += n;
                // What runs: the leaf if it is ours, else the first frame of ours.
                let first = frames.iter().position(|f| !is_system(f)).unwrap_or(0);
                *self.what.entry(frames[first].to_string()).or_default() += n;
                let chain: Vec<&str> = frames[first..].iter().take(5).copied().collect();
                *self.chains.entry(chain.join("  <-  ")).or_default() += n;
            }
        }
    }

    pub fn merge(&mut self, o: &Stats) {
        self.samples += o.samples;
        self.busy += o.busy;
        self.threads += o.threads;
        for k in 0..7 {
            self.wait_kind[k] += o.wait_kind[k];
        }
        for (dst, src) in [
            (&mut self.self_all, &o.self_all),
            (&mut self.self_busy, &o.self_busy),
            (&mut self.inclusive, &o.inclusive),
            (&mut self.what, &o.what),
            (&mut self.chains, &o.chains),
        ] {
            for (k, v) in src {
                *dst.entry(k.clone()).or_default() += v;
            }
        }
        for (k, v) in &o.waits {
            *self.waits.entry(k.clone()).or_default() += v;
        }
    }
}

pub struct ThreadStats {
    pub tid: u32,
    pub group: String,
    pub stats: Stats,
}

pub struct Analysis {
    pub threads: Vec<ThreadStats>,
    /// Threads of one description together, busiest first.
    pub groups: Vec<(String, Stats)>,
    pub all: Stats,
    pub focus: Option<(u64, Map, Map)>,
}

pub fn frames_of<'a>(p: &'a Profile, ids: &[u32]) -> Vec<&'a str> {
    ids.iter().map(|&i| p.names[i as usize].as_str()).collect()
}

pub fn analyze(p: &Profile, o: &Options) -> Analysis {
    let mut threads = Vec::new();
    let mut focus = o.focus.as_ref().map(|_| (0u64, Map::new(), Map::new()));
    for t in &p.threads {
        let mut s = Stats { threads: 1, ..Default::default() };
        for (ids, n) in &t.stacks {
            let frames = frames_of(p, ids);
            s.add(&frames, *n, &o.fence);
            if let (Some(f), Some(fo)) = (&o.focus, focus.as_mut())
                && let Some(i) = frames.iter().rposition(|x| x.contains(f.as_str()))
            {
                fo.0 += n;
                let below = if i > 0 { frames[i - 1].to_string() } else { "(itself)".into() };
                *fo.1.entry(below).or_default() += n;
                let leaf = match classify(&frames, &o.fence) {
                    Some(w) => format!("[{}] {}", w.kind.name(), w.site),
                    None => frames[0].to_string(),
                };
                *fo.2.entry(leaf).or_default() += n;
            }
        }
        threads.push(ThreadStats { tid: t.tid, group: t.group.clone(), stats: s });
    }
    let mut groups: HashMap<String, Stats> = HashMap::new();
    let mut all = Stats::default();
    for t in &threads {
        groups.entry(t.group.clone()).or_default().merge(&t.stats);
        all.merge(&t.stats);
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by(|a, b| b.1.busy.cmp(&a.1.busy).then(b.1.samples.cmp(&a.1.samples)).then(a.0.cmp(&b.0)));
    Analysis { threads, groups, all, focus }
}

pub fn top<K: Ord, V: Copy + Ord>(map: &HashMap<K, V>, n: usize) -> Vec<(&K, V)> {
    let mut v: Vec<_> = map.iter().map(|(k, v)| (k, *v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.truncate(n);
    v
}

fn pct(a: u64, b: u64) -> f64 {
    100.0 * a as f64 / b.max(1) as f64
}

/// "lock 12.0% event 3.1%" for the kinds that have samples.
pub fn kinds_line(s: &Stats) -> String {
    let mut out = String::new();
    for k in Kind::ALL {
        let n = s.wait_kind[k as usize];
        if n > 0 {
            let _ = write!(out, "{}{} {:.1}%", if out.is_empty() { "" } else { ", " }, k.name(), pct(n, s.samples));
        }
    }
    out
}

/// The "waits by site" table: thread-ms/s, % of the samples of `s`, kind, site.
pub fn waits_table(r: &mut String, p: &Profile, s: &Stats, n: usize, indent: &str) {
    let _ = writeln!(r, "{indent}{:>9}  {:>6}  {:<7}  site", "ms/s", "%time", "kind");
    for ((site, kind), c) in top(&s.waits, n) {
        let _ = writeln!(r, "{indent}{:>9.1}  {:>5.1}%  {:<7}  {site}", p.ms_per_s(c), pct(c, s.samples), kind.name());
    }
}

pub fn report(p: &Profile, a: &Analysis, o: &Options) -> String {
    let mut r = String::new();
    let secs = p.seconds;
    let _ = writeln!(r, "kitty6-prof: pid {}, {} rounds in {secs} s ({:.0} Hz achieved of {})", p.pid, p.rounds, p.rounds as f64 / secs, p.hz);
    let _ = writeln!(
        r,
        "sampler: CPU {:.1}% of one core ({:.2} s; busy {:.1}% of the wall time); {} walks, {:.0}% of samples reused (thread had not run);\n         \
         a walked thread is suspended {:.0} us on average, {:.2}% of the threads' time\n",
        100.0 * p.sampler_cpu_s / secs,
        p.sampler_cpu_s,
        100.0 * p.sampler_busy_s / secs,
        p.walks,
        pct(p.reused, p.walks + p.reused),
        1e6 * p.stall_s / p.walks.max(1) as f64,
        100.0 * p.stall_s / (secs * a.threads.len().max(1) as f64),
    );
    let _ = writeln!(r, "== threads (running = samples not in a wait; waits split by kind)");
    for (name, s) in &a.groups {
        let _ = writeln!(
            r,
            "  {:<36} x{:<3} {:>6} samples  running {:>5.1}%  waiting {:>5.1}%  {}",
            name,
            s.threads,
            s.samples,
            pct(s.busy, s.samples),
            pct(s.waiting(), s.samples),
            kinds_line(s)
        );
    }
    let _ = writeln!(r, "\n== all threads: what they run or wait in (first frame of ours)");
    for (k, v) in top(&a.all.what, o.top) {
        let _ = writeln!(r, "  {:>6.1}%  {k}", pct(v, a.all.samples));
    }
    let _ = writeln!(r, "\n== all threads: waits by site (thread-ms per second; % of all samples)");
    waits_table(&mut r, p, &a.all, o.top, "  ");
    for (name, s) in a.groups.iter().take(12) {
        let _ = writeln!(
            r,
            "\n== {name} (x{}, {} samples, running {:.1}%, waiting {:.1}%: {})",
            s.threads,
            s.samples,
            pct(s.busy, s.samples),
            pct(s.waiting(), s.samples),
            kinds_line(s)
        );
        let _ = writeln!(r, "  -- what (first frame of ours):");
        for (k, v) in top(&s.what, 10) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", pct(v, s.samples));
        }
        if !s.waits.is_empty() {
            let _ = writeln!(r, "  -- waits by site:");
            waits_table(&mut r, p, s, 8, "    ");
        }
        let _ = writeln!(r, "  -- chains:");
        for (k, v) in top(&s.chains, 8) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", pct(v, s.samples));
        }
        let _ = writeln!(r, "  -- inclusive:");
        for (k, v) in top(&s.inclusive, 12) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", pct(v, s.samples));
        }
    }
    if let (Some(f), Some(fo)) = (&o.focus, &a.focus) {
        let _ = writeln!(r, "\n== focus: {f} ({} samples under it, {:.1} threads busy in it on average)", fo.0, fo.0 as f64 / p.rounds.max(1) as f64);
        let _ = writeln!(r, "  -- what it calls (frame just below it):");
        for (k, v) in top(&fo.1, 25) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", pct(v, fo.0));
        }
        let _ = writeln!(r, "  -- leaves under it:");
        for (k, v) in top(&fo.2, 25) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", pct(v, fo.0));
        }
    }
    r
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn w(frames: &[&str]) -> Option<(String, Kind)> {
        classify(frames, &[]).map(|w| (w.site, w.kind))
    }

    #[test]
    fn running_code_is_not_a_wait() {
        assert_eq!(w(&["victory_gpu::host::submit", "victory::main"]), None);
        // In ntdll but not blocking (a heap call).
        assert_eq!(w(&["ntdll!RtlAllocateHeap", "victory_gpu::host::submit"]), None);
        // The fast path of a critical section.
        assert_eq!(w(&["ntdll!RtlEnterCriticalSection", "victory_gpu::x"]), None);
    }

    #[test]
    fn srw_lock_waits_go_to_the_caller() {
        let s = ["ntdll!NtWaitForAlertByThreadId", "ntdll!RtlAcquireSRWLockExclusive", "victory_gpu::shadow::exact", "victory_gpu::list::record"];
        assert_eq!(w(&s), Some(("victory_gpu::shadow::exact".into(), Kind::Lock)));
    }

    #[test]
    fn critical_section_slow_path_is_a_lock() {
        let s = ["ntdll!NtWaitForSingleObject", "ntdll!RtlpWaitOnCriticalSection", "ntdll!RtlEnterCriticalSection", "title.exe+0x1234"];
        assert_eq!(w(&s), Some(("title.exe+0x1234".into(), Kind::Lock)));
    }

    #[test]
    fn rust_std_mutex_and_condvar() {
        let m = ["ntdll!NtWaitForAlertByThreadId", "ntdll!RtlWaitOnAddress", "kernelbase!WaitOnAddress", "std::sys::sync::mutex::futex::Mutex::lock_contended", "std::sync::poison::mutex::Mutex::lock", "victory_gpu::land::Pool::state"];
        assert_eq!(w(&m), Some(("victory_gpu::land::Pool::state".into(), Kind::Lock)));
        let c = ["ntdll!NtWaitForAlertByThreadId", "kernelbase!WaitOnAddress", "std::sys::sync::condvar::futex::Condvar::wait_optional_timeout", "std::sync::poison::condvar::Condvar::wait", "victory_gpu::land::worker"];
        assert_eq!(w(&c), Some(("victory_gpu::land::worker".into(), Kind::Event)));
        let park = ["ntdll!NtWaitForAlertByThreadId", "std::thread::park", "crossbeam_channel::context::Context::wait_until", "rayon_core::sleep::Sleep::sleep"];
        assert_eq!(w(&park), Some(("rayon_core::sleep::Sleep::sleep".into(), Kind::Event)));
        let pl = ["ntdll!NtWaitForAlertByThreadId", "parking_lot_core::parking_lot::park", "parking_lot::raw_mutex::RawMutex::lock_slow", "lock_api::mutex::Mutex::lock", "app::cache::get"];
        assert_eq!(w(&pl), Some(("app::cache::get".into(), Kind::Lock)));
        let plc = ["ntdll!NtWaitForAlertByThreadId", "parking_lot_core::parking_lot::park", "parking_lot::condvar::Condvar::wait_until_internal", "app::queue::pop"];
        assert_eq!(w(&plc), Some(("app::queue::pop".into(), Kind::Event)));
    }

    #[test]
    fn sleeps_and_condition_variables() {
        assert_eq!(w(&["ntdll!NtDelayExecution", "kernelbase!SleepEx", "victory_gpu::meter::run"]).unwrap().1, Kind::Sleep);
        assert_eq!(w(&["ntdll!NtDelayExecution", "kernelbase!Sleep", "app::poll"]).unwrap(), ("app::poll".into(), Kind::Sleep));
        // std's sleep: a high-resolution waitable timer.
        let st = ["ntdll!ZwWaitForSingleObject", "kernelbase!WaitForSingleObjectEx", "std::sys::thread::windows::sleep", "target::nap"];
        assert_eq!(w(&st).unwrap(), ("target::nap".into(), Kind::Sleep));
        let cv = ["ntdll!NtWaitForAlertByThreadId", "ntdll!RtlSleepConditionVariableSRW", "kernelbase!SleepConditionVariableSRW", "title.dll+0x10"];
        assert_eq!(w(&cv).unwrap(), ("title.dll+0x10".into(), Kind::Event));
    }

    #[test]
    fn card_fence_waits() {
        // victory's own fence wait: SetEventOnCompletion + WaitForSingleObject in host code.
        let s = ["ntdll!NtWaitForSingleObject", "kernelbase!WaitForSingleObjectEx", "victory_gpu::host::Host::settle", "victory_gpu::host::Host::submit"];
        assert_eq!(w(&s).unwrap(), ("victory_gpu::host::Host::settle".into(), Kind::Fence));
        // A wait inside the driver.
        let d = ["ntdll!NtWaitForSingleObject", "kernelbase!WaitForSingleObjectEx", "nvwgf2umx.dll+0x5521", "d3d12core!CFence::SetEventOnCompletion", "victory_gpu::upq::drain"];
        assert_eq!(w(&d).unwrap(), ("victory_gpu::upq::drain".into(), Kind::Fence));
        // A kernel event elsewhere is an event, unless --fence names it.
        let e = ["ntdll!NtWaitForSingleObject", "kernelbase!WaitForSingleObjectEx", "app::wait_job"];
        assert_eq!(w(&e).unwrap().1, Kind::Event);
        assert_eq!(classify(&e, &["wait_job".into()]).unwrap().kind, Kind::Fence);
    }

    #[test]
    fn pool_workers_that_only_wait() {
        let s = ["ntdll!NtWaitForWorkViaWorkerFactory", "ntdll!TppWorkerThread", "kernel32!BaseThreadInitThunk", "ntdll!RtlUserThreadStart"];
        assert_eq!(w(&s).unwrap(), ("[system] ntdll!TppWorkerThread".into(), Kind::Idle));
    }

    #[test]
    fn folding_numbers() {
        assert_eq!(fold_number("victory-list-3"), "victory-list-#");
        assert_eq!(fold_number("uploads ahead 12"), "uploads ahead #");
        assert_eq!(fold_number("victory-submit"), "victory-submit");
        assert_eq!(fold_number("w7"), "w#");
    }

    pub fn profile(threads: Vec<(u32, &str, Vec<(Vec<&str>, u64)>)>) -> Profile {
        let mut names: Vec<String> = Vec::new();
        let mut ids: HashMap<String, u32> = HashMap::new();
        let mut out = Vec::new();
        for (tid, group, stacks) in threads {
            let stacks = stacks
                .into_iter()
                .map(|(fs, n)| {
                    let v = fs
                        .iter()
                        .map(|f| {
                            *ids.entry(f.to_string()).or_insert_with(|| {
                                names.push(f.to_string());
                                (names.len() - 1) as u32
                            })
                        })
                        .collect();
                    (v, n)
                })
                .collect();
            out.push(ThreadData { tid, group: group.to_string(), described: !group.starts_with('('), stacks });
        }
        Profile { pid: 1, seconds: 1.0, hz: 100.0, rounds: 100, names, threads: out, sampler_cpu_s: 0.01, sampler_busy_s: 0.01, stall_s: 0.0, walks: 1, reused: 0 }
    }

    #[test]
    fn waits_by_site_in_ms_per_second() {
        let p = profile(vec![
            (1, "victory-list-#", vec![
                (vec!["ntdll!NtWaitForAlertByThreadId", "ntdll!RtlAcquireSRWLockExclusive", "victory_gpu::shadow::exact", "victory_gpu::list::record"], 75),
                (vec!["victory_gpu::list::record"], 25),
            ]),
            (2, "victory-list-#", vec![(vec!["victory_gpu::list::record"], 100)]),
        ]);
        let a = analyze(&p, &Options::default());
        assert_eq!(a.groups.len(), 1);
        let s = &a.groups[0].1;
        assert_eq!((s.samples, s.busy, s.threads), (200, 125, 2));
        assert_eq!(s.wait_kind[Kind::Lock as usize], 75);
        let c = s.waits[&("victory_gpu::shadow::exact".to_string(), Kind::Lock)];
        assert_eq!(p.ms_per_s(c), 750.0);
        let r = report(&p, &a, &Options::default());
        assert!(r.contains("750.0   37.5%  lock     victory_gpu::shadow::exact"), "{r}");
    }
}
