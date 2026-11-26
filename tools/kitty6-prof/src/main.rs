//! kitty6-prof: a sampling profiler for a running process (victory.exe by
//! default). Every few milliseconds each thread is suspended, its call stack
//! walked with dbghelp (the diag build's PDB names our functions; the title's
//! code shows as module+offset), and resumed. At the end it says, per thread
//! and over all, where the samples fell: the leaf (what runs or waits now)
//! and the callers that hold the most samples, so a thread that waits shows
//! what it waits in and who asked.
//!
//!   kitty6-prof [--pid N | --name victory.exe] [--seconds 20] [--hz 100]
//!               [--top 25] [--out report.txt]

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CONTEXT, GetThreadContext, STACKFRAME64, StackWalk64, SYMBOL_INFOW, SymFromAddrW, SymFunctionTableAccess64,
    SymGetModuleBase64, SymInitializeW, SymRefreshModuleList, SymSetOptions, AddrModeFlat,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W, Process32FirstW,
    Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32,
    Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
    GetThreadDescription, OpenProcess, OpenThread, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, ResumeThread,
    SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

const IMAGE_FILE_MACHINE_AMD64: u32 = 0x8664;
const CONTEXT_FULL: u32 = 0x0010_000B;
const MAX_FRAMES: usize = 48;

#[repr(C, align(16))]
struct Ctx(CONTEXT);

struct Args {
    pid: Option<u32>,
    name: String,
    seconds: f64,
    hz: f64,
    top: usize,
    out: Option<String>,
    focus: Option<String>,
}

fn args() -> Args {
    let mut a = Args { pid: None, name: "victory.exe".into(), seconds: 20.0, hz: 100.0, top: 25, out: None, focus: None };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        match k.as_str() {
            "--pid" => a.pid = v().parse().ok(),
            "--name" => a.name = v(),
            "--seconds" => a.seconds = v().parse().unwrap_or(20.0),
            "--hz" => a.hz = v().parse().unwrap_or(100.0),
            "--top" => a.top = v().parse().unwrap_or(25),
            "--out" => a.out = Some(v()),
            "--focus" => a.focus = Some(v()),
            _ => {
                eprintln!("usage: kitty6-prof [--pid N | --name victory.exe] [--seconds 20] [--hz 100] [--top 25] [--out file]");
                std::process::exit(2);
            }
        }
    }
    a
}

fn wide(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

fn find_pid(name: &str) -> Option<u32> {
    // SAFETY: plain snapshot walking with a sized entry.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                if wide(&e.szExeFile).eq_ignore_ascii_case(name) {
                    found = Some(e.th32ProcessID);
                    break;
                }
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        found
    }
}

fn threads(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    // SAFETY: as above.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = size_of::<THREADENTRY32>() as u32;
        if Thread32First(snap, &mut e) != 0 {
            loop {
                if e.th32OwnerProcessID == pid {
                    out.push(e.th32ThreadID);
                }
                if Thread32Next(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// Modules of the process: (base, end, name), for addresses without symbols.
fn modules(pid: u32) -> Vec<(u64, u64, String)> {
    let mut out = Vec::new();
    // SAFETY: as above.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        let mut e: MODULEENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<MODULEENTRY32W>() as u32;
        if Module32FirstW(snap, &mut e) != 0 {
            loop {
                let base = e.modBaseAddr as u64;
                out.push((base, base + u64::from(e.modBaseSize), wide(&e.szModule)));
                if Module32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// Modules of the process with their full paths.
fn module_paths(pid: u32) -> Vec<(u64, u64, String)> {
    let mut out = Vec::new();
    // SAFETY: as in `modules`.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        let mut e: MODULEENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<MODULEENTRY32W>() as u32;
        if Module32FirstW(snap, &mut e) != 0 {
            loop {
                let base = e.modBaseAddr as u64;
                out.push((base, base + u64::from(e.modBaseSize), wide(&e.szExePath)));
                if Module32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

fn thread_name(h: HANDLE, tid: u32) -> String {
    // SAFETY: the description is a LocalAlloc'd wide string we only read.
    unsafe {
        let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
        if GetThreadDescription(h, &mut p) >= 0 && !p.is_null() {
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            windows_sys::Win32::Foundation::LocalFree(p as *mut c_void);
            if !s.is_empty() {
                // Numbered workers are one kind.
                return s.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-').to_owned() + "#";
            }
        }
    }
    format!("(unnamed {tid})")
}

struct Symbols {
    process: HANDLE,
    cache: HashMap<u64, String>,
    modules: Vec<(u64, u64, String)>,
}

impl Symbols {
    fn name(&mut self, addr: u64) -> String {
        if let Some(s) = self.cache.get(&addr) {
            return s.clone();
        }
        let s = self.resolve(addr);
        self.cache.insert(addr, s.clone());
        s
    }

    fn resolve(&self, addr: u64) -> String {
        #[repr(C)]
        struct Sym {
            info: SYMBOL_INFOW,
            name: [u16; 512],
        }
        // SAFETY: a sized symbol buffer dbghelp fills.
        unsafe {
            let mut s: Sym = std::mem::zeroed();
            s.info.SizeOfStruct = size_of::<SYMBOL_INFOW>() as u32;
            s.info.MaxNameLen = 512;
            let mut disp = 0u64;
            if SymFromAddrW(self.process, addr, &mut disp, &mut s.info) != 0 {
                let len = (s.info.NameLen as usize).min(512);
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(s.info.Name.as_ptr(), len));
                return shorten(&name);
            }
        }
        match self.modules.iter().find(|(b, e, _)| addr >= *b && addr < *e) {
            Some((b, _, m)) => format!("{m}+{:#x}", addr - b),
            None => format!("{addr:#x}"),
        }
    }
}

/// Drops generic arguments and hashes from Rust names.
fn shorten(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Functions that only wait: a stack whose leaf is one of these is waiting,
/// and the first frame above them says on what.
fn is_wait(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["ntwaitfor", "ntdelayexecution", "zwdelayexecution", "tpworkonbehalf", "rtldllshutdown", "ntwaitforalert", "ntremovelo", "ntuserget", "ntusermsgwait", "zwwaitfor", "ntalertthread",
        "rtlsleepcondition", "rtlacquiresrw", "rtlpwaitoncritical", "rtlpwaitonaddress", "ntwaitforalert",
        "sleepconditionvariable", "waitforsingleobject", "waitformultipleobjects", "sleepex", "waitonaddress",
        "ntsignalandwait", "rtlenter", "ntyieldexecution", "parking_lot", "futex", "std::sys::sync", "std::thread::park",
        "kernelbase!", "ntdll!", "kernel32!"]
        .iter()
        .any(|w| n.contains(w))
        || n.starts_with("ntdll.dll+")
        || n.starts_with("kernelbase.dll+")
        || n.starts_with("kernel32.dll+")
        || n.starts_with("win32u.dll+")
}

#[derive(Default)]
struct Stats {
    samples: u64,
    leaf: HashMap<String, u64>,
    /// First frame of ours (not a wait primitive) on the stack: what the
    /// thread is doing or waiting for.
    what: HashMap<String, u64>,
    inclusive: HashMap<String, u64>,
    /// "what <- caller <- caller" chains.
    chains: HashMap<String, u64>,
    waiting: u64,
}

fn walk(process: HANDLE, thread: HANDLE) -> Vec<u64> {
    // SAFETY: the thread is suspended; the context is 16-byte aligned.
    unsafe {
        let mut ctx: Box<Ctx> = Box::new(std::mem::zeroed());
        ctx.0.ContextFlags = CONTEXT_FULL;
        if GetThreadContext(thread, &mut ctx.0) == 0 {
            return Vec::new();
        }
        let mut frame: STACKFRAME64 = std::mem::zeroed();
        frame.AddrPC.Offset = ctx.0.Rip;
        frame.AddrPC.Mode = AddrModeFlat;
        frame.AddrFrame.Offset = ctx.0.Rbp;
        frame.AddrFrame.Mode = AddrModeFlat;
        frame.AddrStack.Offset = ctx.0.Rsp;
        frame.AddrStack.Mode = AddrModeFlat;
        let mut out = Vec::with_capacity(MAX_FRAMES);
        while out.len() < MAX_FRAMES {
            if StackWalk64(
                IMAGE_FILE_MACHINE_AMD64,
                process,
                thread,
                &mut frame,
                (&mut ctx.0 as *mut CONTEXT).cast(),
                None,
                Some(SymFunctionTableAccess64),
                Some(SymGetModuleBase64),
                None,
            ) == 0
                || frame.AddrPC.Offset == 0
            {
                break;
            }
            out.push(frame.AddrPC.Offset);
        }
        out
    }
}

fn top(map: &HashMap<String, u64>, n: usize) -> Vec<(&String, &u64)> {
    let mut v: Vec<_> = map.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    v.truncate(n);
    v
}

/// `--resolve <image> <offset>...`: names offsets of an image file with its
/// PDB, loaded into this process (for addresses the live attach left bare).
fn resolve_offline(rest: &[String]) {
    use windows_sys::Win32::System::Diagnostics::Debug::{SymLoadModuleExW, SymGetLineFromAddrW64, IMAGEHLP_LINEW64};
    let image = &rest[0];
    let w: Vec<u16> = image.encode_utf16().chain([0]).collect();
    let me = 0x1234usize as HANDLE;
    unsafe {
        SymSetOptions(0x2 | 0x10 | 0x200 | 0x80000); // undname, load lines, fail critical, no prompts
        if SymInitializeW(me, std::ptr::null(), FALSE) == 0 { eprintln!("SymInitialize failed"); return; }
        let base = 0x1_4000_0000u64;
        let got = SymLoadModuleExW(me, std::ptr::null_mut(), w.as_ptr(), std::ptr::null(), base, 0, std::ptr::null(), 0);
        if got == 0 { eprintln!("could not load {image}: {}", std::io::Error::last_os_error()); return; }
        let syms = Symbols { process: me, cache: HashMap::new(), modules: Vec::new() };
        for o in &rest[1..] {
            let off = u64::from_str_radix(o.trim_start_matches("0x"), 16).unwrap_or(0);
            let mut line: IMAGEHLP_LINEW64 = std::mem::zeroed();
            line.SizeOfStruct = size_of::<IMAGEHLP_LINEW64>() as u32;
            let mut d = 0u32;
            let at = if SymGetLineFromAddrW64(me, got + off, &mut d, &mut line) != 0 {
                let mut n = 0; while *line.FileName.add(n) != 0 { n += 1; }
                format!("  {}:{}", String::from_utf16_lossy(std::slice::from_raw_parts(line.FileName, n)), line.LineNumber)
            } else { String::new() };
            println!("{o}  {}{at}", syms.resolve(got + off));
        }
    }
}

fn main() {
    let all: Vec<String> = std::env::args().skip(1).collect();
    if all.first().map(String::as_str) == Some("--resolve") {
        return resolve_offline(&all[1..]);
    }
    let a = args();
    let pid = a.pid.or_else(|| find_pid(&a.name)).unwrap_or_else(|| {
        eprintln!("no process {}", a.name);
        std::process::exit(1)
    });
    // SAFETY: handles opened and closed here; dbghelp is used on this thread only.
    let process = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, FALSE, pid) };
    if process.is_null() {
        eprintln!("cannot open process {pid}");
        std::process::exit(1);
    }
    unsafe {
        const SYMOPT_UNDNAME: u32 = 0x2;
        const SYMOPT_DEFERRED_LOADS: u32 = 0x4;
        const SYMOPT_FAIL_CRITICAL_ERRORS: u32 = 0x200;
        const SYMOPT_NO_PROMPTS: u32 = 0x80000;
        SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_FAIL_CRITICAL_ERRORS | SYMOPT_NO_PROMPTS);
        if SymInitializeW(process, std::ptr::null(), FALSE) == 0 {
            eprintln!("dbghelp could not attach (symbols will be module+offset)");
        }
        // Each module loaded by its file, so its PDB beside it is found.
        for (base, end, path) in module_paths(pid) {
            let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
            windows_sys::Win32::System::Diagnostics::Debug::SymLoadModuleExW(
                process, std::ptr::null_mut(), w.as_ptr(), std::ptr::null(), base, (end - base) as u32, std::ptr::null(), 0,
            );
        }
        let _ = SymRefreshModuleList;
    }
    // Names from a session of their own with every module loaded by its file
    // at its base, not deferred (the live session left our PDB unread).
    let names_handle = 0x5678usize as HANDLE;
    unsafe {
        SymSetOptions(0x2 | 0x200 | 0x80000);
        SymInitializeW(names_handle, std::ptr::null(), FALSE);
        for (base, end, path) in module_paths(pid) {
            let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
            windows_sys::Win32::System::Diagnostics::Debug::SymLoadModuleExW(
                names_handle, std::ptr::null_mut(), w.as_ptr(), std::ptr::null(), base, (end - base) as u32, std::ptr::null(), 0,
            );
        }
    }
    let mut syms = Symbols { process: names_handle, cache: HashMap::new(), modules: modules(pid) };
    let mut focus: (u64, HashMap<String, u64>, HashMap<String, u64>) = (0, HashMap::new(), HashMap::new());
    let mut stats: HashMap<String, Stats> = HashMap::new();
    let mut names: HashMap<u32, String> = HashMap::new();
    let period = Duration::from_secs_f64(1.0 / a.hz.max(1.0));
    let end = Instant::now() + Duration::from_secs_f64(a.seconds);
    let mut rounds = 0u64;
    let mut last_list = Instant::now() - Duration::from_secs(10);
    let mut tids = Vec::new();
    eprintln!("sampling pid {pid} for {} s at {} Hz…", a.seconds, a.hz);
    while Instant::now() < end {
        let t0 = Instant::now();
        if last_list.elapsed() > Duration::from_secs(1) {
            tids = threads(pid);
            last_list = Instant::now();
        }
        for &tid in &tids {
            // SAFETY: a thread of the target, suspended only while walked.
            unsafe {
                let h = OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION,
                    FALSE,
                    tid,
                );
                if h.is_null() {
                    continue;
                }
                let name = names.entry(tid).or_insert_with(|| thread_name(h, tid)).clone();
                if SuspendThread(h) == u32::MAX {
                    CloseHandle(h);
                    continue;
                }
                let stack = walk(process, h);
                ResumeThread(h);
                CloseHandle(h);
                if stack.is_empty() {
                    continue;
                }
                let frames: Vec<String> = stack.iter().map(|&p| syms.name(p)).collect();
                if let Some(f) = &a.focus
                    && let Some(i) = frames.iter().rposition(|x| x.contains(f.as_str()))
                {
                    focus.0 += 1;
                    let below = if i > 0 { frames[i - 1].clone() } else { "(itself)".into() };
                    *focus.1.entry(below).or_default() += 1;
                    let leaf = if is_wait(&frames[0]) { format!("[wait] {}", frames.iter().find(|x| !is_wait(x)).cloned().unwrap_or_default()) } else { frames[0].clone() };
                    *focus.2.entry(leaf).or_default() += 1;
                }
                let s = stats.entry(name).or_default();
                s.samples += 1;
                *s.leaf.entry(frames[0].clone()).or_default() += 1;
                let first_ours = frames.iter().position(|f| !is_wait(f)).unwrap_or(0);
                if first_ours > 0 {
                    s.waiting += 1;
                }
                let what = if first_ours > 0 {
                    format!("[wait] {}", frames[first_ours])
                } else {
                    frames[0].clone()
                };
                *s.what.entry(what.clone()).or_default() += 1;
                let chain: Vec<&str> = frames[first_ours..].iter().take(5).map(String::as_str).collect();
                *s.chains.entry(format!("{}{}", if first_ours > 0 { "[wait] " } else { "" }, chain.join("  <-  "))).or_default() += 1;
                let mut seen = std::collections::HashSet::new();
                for f in &frames {
                    if seen.insert(f) {
                        *s.inclusive.entry(f.clone()).or_default() += 1;
                    }
                }
            }
        }
        rounds += 1;
        if let Some(rest) = period.checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    // Report.
    let mut r = String::new();
    use std::fmt::Write;
    let secs = a.seconds;
    let _ = writeln!(r, "kitty6-prof: pid {pid}, {rounds} rounds in {secs} s ({:.0} Hz achieved)\n", rounds as f64 / secs);
    let mut kinds: Vec<_> = stats.iter().collect();
    // Busiest first: samples not waiting.
    kinds.sort_by(|a, b| (b.1.samples - b.1.waiting).cmp(&(a.1.samples - a.1.waiting)));
    let _ = writeln!(r, "== threads (running share = samples not in a wait primitive)");
    for (name, s) in &kinds {
        let _ = writeln!(r, "  {:<40} {:>6} samples  running {:>5.1}%", name, s.samples, 100.0 * (s.samples - s.waiting) as f64 / s.samples.max(1) as f64);
    }
    let mut all = Stats::default();
    for (_, s) in &kinds {
        for (k, v) in &s.what {
            *all.what.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &s.inclusive {
            *all.inclusive.entry(k.clone()).or_default() += v;
        }
        all.samples += s.samples;
    }
    let _ = writeln!(r, "\n== all threads: what they run or wait in (first frame of ours)");
    for (k, v) in top(&all.what, a.top) {
        let _ = writeln!(r, "  {:>6.1}%  {k}", 100.0 * *v as f64 / all.samples.max(1) as f64);
    }
    for (name, s) in kinds.iter().take(12) {
        let _ = writeln!(r, "\n== {name} ({} samples, running {:.1}%)", s.samples, 100.0 * (s.samples - s.waiting) as f64 / s.samples.max(1) as f64);
        let _ = writeln!(r, "  -- what (first frame of ours):");
        for (k, v) in top(&s.what, 10) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", 100.0 * *v as f64 / s.samples.max(1) as f64);
        }
        let _ = writeln!(r, "  -- chains:");
        for (k, v) in top(&s.chains, 8) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", 100.0 * *v as f64 / s.samples.max(1) as f64);
        }
        let _ = writeln!(r, "  -- inclusive:");
        for (k, v) in top(&s.inclusive, 12) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", 100.0 * *v as f64 / s.samples.max(1) as f64);
        }
    }
    if let Some(f) = &a.focus {
        let _ = writeln!(r, "
== focus: {f} ({} samples under it, {:.1} threads busy in it on average)", focus.0, focus.0 as f64 / rounds.max(1) as f64);
        let _ = writeln!(r, "  -- what it calls (frame just below it):");
        for (k, v) in top(&focus.1, 25) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", 100.0 * *v as f64 / focus.0.max(1) as f64);
        }
        let _ = writeln!(r, "  -- leaves under it:");
        for (k, v) in top(&focus.2, 25) {
            let _ = writeln!(r, "    {:>6.1}%  {k}", 100.0 * *v as f64 / focus.0.max(1) as f64);
        }
    }
    print!("{r}");
    if let Some(out) = a.out {
        let _ = std::fs::write(&out, &r);
        eprintln!("written to {out}");
    }
    unsafe { CloseHandle(process) };
}
