//! kitty6-prof: a sampling profiler for a running process (victory.exe by
//! default). Every few milliseconds each thread is suspended, its call stack
//! walked with dbghelp (the diag build's PDB names our functions; the title's
//! code shows as module+offset), and resumed. Stacks are kept as raw
//! addresses while sampling (cheap) and named once at the end. The report
//! says, per thread and over all, where the samples fell: the leaf, the
//! callers that hold the most samples, and for samples that wait, on what
//! (lock, event, card fence, sleep) and from where (the first frame of ours
//! above the wait primitive).
//!
//! See `USAGE` below for the options.

mod analysis;

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddrModeFlat, CONTEXT, GetThreadContext, STACKFRAME64, SYMBOL_INFOW, StackWalk64, SymFromAddrW,
    SymFunctionTableAccess64, SymGetModuleBase64, SymInitializeW, SymLoadModuleExW, SymSetOptions,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W, Process32FirstW,
    Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32,
    Thread32First, Thread32Next,
};
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;
use windows_sys::Win32::System::Threading::{
    GetThreadDescription, OpenProcess, OpenThread, PROCESS_QUERY_INFORMATION,
    PROCESS_VM_READ, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION,
    THREAD_SUSPEND_RESUME,
};

use analysis::{Profile, ThreadData};

const IMAGE_FILE_MACHINE_AMD64: u32 = 0x8664;
const CONTEXT_FULL: u32 = 0x0010_000B;
const MAX_FRAMES: usize = 48;
/// Cycles a thread may show after its suspension ends without having run
/// code of its own (measured: 2.5-8k; a wake and wait again costs more).
const APC_TAIL: u64 = 12_000;

const USAGE: &str = "\
kitty6-prof: samples every thread's call stack of a running process

usage:
  kitty6-prof [--pid N | --name victory.exe] [--seconds 20] [--hz 100]
              [--top 25] [--out report.txt] [--focus NAME]
  kitty6-prof --resolve <image.exe> 0xOFFSET...

sampling:
  --pid N          process id (default: the first process named --name)
  --name EXE       process name (default victory.exe)
  --seconds S      how long to sample (default 20)
  --hz H           samples per thread per second (default 100; 50 is plenty
                   for a 20 s run and halves the sampler's cost)
report:
  --top N          rows in the all-threads tables (default 25)
  --out FILE       also write the text report to FILE
  --focus NAME     what runs under the frames whose name contains NAME
  --fence PAT      extra frame-name pattern (repeatable) marking a kernel
                   event wait as a card fence wait (default patterns cover
                   victory_gpu::host/fence/queue and *fence*/*settle*)

The report shows per thread: running share, waits by site (the first frame of
ours above the wait primitive, with the wait kind: lock / event / fence /
sleep / idle / message), what it runs, call chains and inclusive functions.
Wait times are thread-ms per second of wall time (a thread waiting the whole
time is 1000 ms/s) and % of that thread's samples.
";

#[repr(C, align(16))]
struct Ctx(CONTEXT);

struct Args {
    pid: Option<u32>,
    name: String,
    seconds: f64,
    hz: f64,
    opts: analysis::Options,
    out: Option<String>,
}

fn usage_exit(code: i32) -> ! {
    if code == 0 {
        print!("{USAGE}");
    } else {
        eprint!("{USAGE}");
    }
    std::process::exit(code)
}

fn args(list: &[String]) -> Args {
    let mut a = Args {
        pid: None,
        name: "victory.exe".into(),
        seconds: 20.0,
        hz: 100.0,
        opts: analysis::Options::default(),
        out: None,
    };
    let mut it = list.iter().cloned();
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| usage_exit(2));
        match k.as_str() {
            "--pid" => a.pid = v().parse().ok(),
            "--name" => a.name = v(),
            "--seconds" => a.seconds = v().parse().unwrap_or(20.0),
            "--hz" => a.hz = v().parse().unwrap_or(100.0),
            "--top" => a.opts.top = v().parse().unwrap_or(25),
            "--out" => a.out = Some(v()),
            "--focus" => a.opts.focus = Some(v()),
            "--fence" => a.opts.fence.push(v().to_ascii_lowercase()),
            "--help" | "-h" => usage_exit(0),
            _ => {
                eprintln!("unknown option {k}");
                usage_exit(2)
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

/// Modules of the process: (base, end, file name, full path). The first is
/// the exe.
fn modules(pid: u32) -> Vec<(u64, u64, String, String)> {
    let mut out = Vec::new();
    // SAFETY: as above.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        let mut e: MODULEENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<MODULEENTRY32W>() as u32;
        if Module32FirstW(snap, &mut e) != 0 {
            loop {
                let base = e.modBaseAddr as u64;
                out.push((base, base + u64::from(e.modBaseSize), wide(&e.szModule), wide(&e.szExePath)));
                if Module32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// The thread's description (SetThreadDescription; Rust's thread names),
/// numbered workers folded into one kind ("victory-list-3" -> "victory-list-#").
fn thread_description(h: HANDLE) -> Option<String> {
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
                return Some(analysis::fold_number(&s));
            }
        }
    }
    None
}

struct Symbols {
    process: HANDLE,
    /// (base, end, file name); the first is the exe, whose names go bare.
    /// Frames of other modules are "module!name", or "module.dll+0xoffset"
    /// without a symbol.
    modules: Vec<(u64, u64, String)>,
}

impl Symbols {
    fn resolve(&self, addr: u64) -> String {
        #[repr(C)]
        struct Sym {
            info: SYMBOL_INFOW,
            name: [u16; 512],
        }
        let module = self.modules.iter().enumerate().find(|(_, (b, e, _))| addr >= *b && addr < *e);
        // SAFETY: a sized symbol buffer dbghelp fills.
        unsafe {
            let mut s: Sym = std::mem::zeroed();
            s.info.SizeOfStruct = size_of::<SYMBOL_INFOW>() as u32;
            s.info.MaxNameLen = 512;
            let mut disp = 0u64;
            if SymFromAddrW(self.process, addr, &mut disp, &mut s.info) != 0 {
                let len = (s.info.NameLen as usize).min(512);
                let name = shorten(&String::from_utf16_lossy(std::slice::from_raw_parts(s.info.Name.as_ptr(), len)));
                return match module {
                    Some((i, (_, _, m))) if i > 0 => {
                        let stem = m.rsplit_once('.').map_or(m.as_str(), |(s, _)| s);
                        format!("{}!{name}", stem.to_ascii_lowercase())
                    }
                    _ => name,
                };
            }
        }
        match module {
            Some((_, (b, _, m))) => format!("{m}+{:#x}", addr - b),
            None => format!("{addr:#x}"),
        }
    }
}

/// Drops generic arguments and hashes from Rust names; no ';' (the folded
/// format's separator) or line breaks.
fn shorten(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ';' | '\n' | '\r' if depth == 0 => out.push(':'),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Reads of the target's memory for the stack walk, a page at a time and
/// cached: pages of module images (code, unwind data) for the whole run,
/// other pages (the stack) for one walk. dbghelp otherwise reads a few bytes
/// per call, a system call each.
struct PageCache {
    images: Vec<(u64, u64)>,
    keep: HashMap<u64, Option<Box<[u8; 4096]>>>,
    walk: HashMap<u64, Option<Box<[u8; 4096]>>>,
    reads: u64,
}

thread_local! {
    static PAGES: std::cell::RefCell<PageCache> =
        std::cell::RefCell::new(PageCache { images: Vec::new(), keep: HashMap::new(), walk: HashMap::new(), reads: 0 });
}

unsafe extern "system" fn read_memory(process: HANDLE, at: u64, buf: *mut c_void, size: u32, read: *mut u32) -> windows_sys::core::BOOL {
    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    PAGES.with_borrow_mut(|c| {
        let mut done = 0u32;
        while done < size {
            let addr = at + u64::from(done);
            let page = addr & !0xfff;
            let image = c.images.iter().any(|(b, e)| page >= *b && page < *e);
            let reads = &mut c.reads;
            let map = if image { &mut c.keep } else { &mut c.walk };
            let got = map.entry(page).or_insert_with(|| {
                *reads += 1;
                let mut p = Box::new([0u8; 4096]);
                let mut n = 0usize;
                // SAFETY: a 4 KiB buffer of ours.
                let ok = unsafe { ReadProcessMemory(process, page as *const c_void, p.as_mut_ptr().cast(), 4096, &mut n) };
                (ok != 0 && n == 4096).then_some(p)
            });
            let Some(p) = got else { break };
            let off = (addr - page) as usize;
            let take = (4096 - off).min((size - done) as usize);
            // SAFETY: the caller's buffer holds `size` bytes.
            unsafe { std::ptr::copy_nonoverlapping(p.as_ptr().add(off), buf.cast::<u8>().add(done as usize), take) };
            done += take as u32;
        }
        if !read.is_null() {
            // SAFETY: the caller's count.
            unsafe { *read = done };
        }
        (done == size) as windows_sys::core::BOOL
    })
}

thread_local! {
    /// Function table entries by return address (12-byte RUNTIME_FUNCTIONs,
    /// copied: dbghelp's own buffer is reused), null for none.
    static FUNCTIONS: std::cell::RefCell<HashMap<u64, Option<Box<[u32; 3]>>>> = std::cell::RefCell::new(HashMap::new());
}

unsafe extern "system" fn function_table(process: HANDLE, pc: u64) -> *mut c_void {
    FUNCTIONS.with_borrow_mut(|m| {
        let e = m.entry(pc).or_insert_with(|| {
            // SAFETY: dbghelp returns null or a RUNTIME_FUNCTION we copy at once.
            let p = unsafe { SymFunctionTableAccess64(process, pc) } as *const [u32; 3];
            (!p.is_null()).then(|| Box::new(unsafe { *p }))
        });
        e.as_mut().map_or(std::ptr::null_mut(), |b| b.as_mut_ptr().cast())
    })
}

unsafe extern "system" fn module_base(process: HANDLE, pc: u64) -> u64 {
    let base = PAGES.with_borrow(|c| c.images.iter().find(|(b, e)| pc >= *b && pc < *e).map(|(b, _)| *b));
    // SAFETY: plain dbghelp query for addresses outside the known modules.
    base.unwrap_or_else(|| unsafe { SymGetModuleBase64(process, pc) })
}

fn walk(process: HANDLE, thread: HANDLE, ctx: &mut Ctx, out: &mut Vec<u64>) {
    out.clear();
    PAGES.with_borrow_mut(|c| c.walk.clear());
    // SAFETY: the thread is suspended; the context is 16-byte aligned.
    unsafe {
        ctx.0 = std::mem::zeroed();
        ctx.0.ContextFlags = CONTEXT_FULL;
        if GetThreadContext(thread, &mut ctx.0) == 0 {
            return;
        }
        let mut frame: STACKFRAME64 = std::mem::zeroed();
        frame.AddrPC.Offset = ctx.0.Rip;
        frame.AddrPC.Mode = AddrModeFlat;
        frame.AddrFrame.Offset = ctx.0.Rbp;
        frame.AddrFrame.Mode = AddrModeFlat;
        frame.AddrStack.Offset = ctx.0.Rsp;
        frame.AddrStack.Mode = AddrModeFlat;
        while out.len() < MAX_FRAMES {
            if StackWalk64(
                IMAGE_FILE_MACHINE_AMD64,
                process,
                thread,
                &mut frame,
                (&mut ctx.0 as *mut CONTEXT).cast(),
                Some(read_memory),
                Some(function_table),
                Some(module_base),
                None,
            ) == 0
                || frame.AddrPC.Offset == 0
            {
                break;
            }
            out.push(frame.AddrPC.Offset);
        }
    }
}

/// `--resolve <image> <offset>...`: names offsets of an image file with its
/// PDB, loaded into this process (for addresses the live attach left bare).
fn resolve_offline(rest: &[String]) {
    use windows_sys::Win32::System::Diagnostics::Debug::{IMAGEHLP_LINEW64, SymGetLineFromAddrW64};
    let Some(image) = rest.first() else { usage_exit(2) };
    let w: Vec<u16> = image.encode_utf16().chain([0]).collect();
    let me = 0x1234usize as HANDLE;
    unsafe {
        SymSetOptions(0x2 | 0x10 | 0x200 | 0x80000); // undname, load lines, fail critical, no prompts
        if SymInitializeW(me, std::ptr::null(), FALSE) == 0 {
            eprintln!("SymInitialize failed");
            return;
        }
        let base = 0x1_4000_0000u64;
        let got = SymLoadModuleExW(me, std::ptr::null_mut(), w.as_ptr(), std::ptr::null(), base, 0, std::ptr::null(), 0);
        if got == 0 {
            eprintln!("could not load {image}: {}", std::io::Error::last_os_error());
            return;
        }
        let syms = Symbols { process: me, modules: Vec::new() };
        for o in &rest[1..] {
            let off = u64::from_str_radix(o.trim_start_matches("0x"), 16).unwrap_or(0);
            let mut line: IMAGEHLP_LINEW64 = std::mem::zeroed();
            line.SizeOfStruct = size_of::<IMAGEHLP_LINEW64>() as u32;
            let mut d = 0u32;
            let at = if SymGetLineFromAddrW64(me, got + off, &mut d, &mut line) != 0 {
                let mut n = 0;
                while *line.FileName.add(n) != 0 {
                    n += 1;
                }
                format!("  {}:{}", String::from_utf16_lossy(std::slice::from_raw_parts(line.FileName, n)), line.LineNumber)
            } else {
                String::new()
            };
            println!("{o}  {}{at}", syms.resolve(got + off));
        }
    }
}

/// This thread's CPU time in cycles (exact, unlike GetProcessTimes, which
/// charges whole clock ticks and misses a thread that wakes, works and
/// sleeps between two ticks — the sampler's pattern).
fn own_cycles() -> u64 {
    use windows_sys::Win32::System::Threading::GetCurrentThread;
    use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;
    let mut c = 0u64;
    // SAFETY: the pseudo-handle of this thread.
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) };
    c
}

/// Cycles per second of the cycle counter, from a short busy spin.
fn cycles_per_second() -> f64 {
    let (t, c) = (Instant::now(), own_cycles());
    while t.elapsed() < Duration::from_millis(30) {
        std::hint::spin_loop();
    }
    (own_cycles() - c) as f64 / t.elapsed().as_secs_f64()
}

/// A thread of the target while sampling: its handle (null once gone) and
/// its stacks as raw addresses, leaf first.
struct Live {
    h: HANDLE,
    group: String,
    described: bool,
    stacks: HashMap<Vec<u64>, u64>,
    /// The last stack walked and the thread's cycle count after it.
    last: Vec<u64>,
    cycles: u64,
    /// Walked last round: the suspension's own tail (the thread finishing
    /// the suspend APC after ResumeThread, a few thousand cycles) may follow.
    walked: bool,
}

/// What sampling took, to say how much it disturbs the target.
#[derive(Default)]
struct Cost {
    rounds: u64,
    /// The sampler's CPU time, s.
    cpu_s: f64,
    /// Wall time spent sampling (not sleeping), s.
    busy_s: f64,
    /// Target threads' time suspended, summed, s, and the walks.
    stall_s: f64,
    walks: u64,
    /// Samples of threads that had not run since their last one.
    reused: u64,
}

fn sample(pid: u32, process: HANDLE, seconds: f64, hz: f64) -> (HashMap<u32, Live>, Cost) {
    let mut live: HashMap<u32, Live> = HashMap::new();
    let mut tids: Vec<u32> = Vec::new();
    let period = Duration::from_secs_f64(1.0 / hz.max(1.0));
    let rate = cycles_per_second();
    let start = Instant::now();
    let end = start + Duration::from_secs_f64(seconds);
    let mut next = start;
    let mut last_list = start - Duration::from_secs(10);
    // SAFETY: a plain C struct, zeroed.
    let mut ctx: Box<Ctx> = Box::new(unsafe { std::mem::zeroed() });
    let mut stack = Vec::with_capacity(MAX_FRAMES);
    let mut cost = Cost::default();
    let mut stall = Duration::ZERO;
    let mut busy = Duration::ZERO;
    let cycles0 = own_cycles();
    while Instant::now() < end {
        let t0 = Instant::now();
        if last_list.elapsed() > Duration::from_secs(1) {
            let now = threads(pid);
            for (tid, l) in live.iter_mut() {
                if !l.h.is_null() && !now.contains(tid) {
                    // SAFETY: our handle, closed once.
                    unsafe { CloseHandle(l.h) };
                    l.h = std::ptr::null_mut();
                }
            }
            for &tid in &now {
                if live.contains_key(&tid) {
                    continue;
                }
                // SAFETY: a handle to a thread of the target, kept until it exits.
                let h = unsafe {
                    OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_LIMITED_INFORMATION, FALSE, tid)
                };
                if h.is_null() {
                    continue;
                }
                let d = thread_description(h);
                live.insert(
                    tid,
                    Live { h, described: d.is_some(), group: d.unwrap_or_else(|| format!("(unnamed {tid})")), stacks: HashMap::new(), last: Vec::new(), cycles: 0, walked: false },
                );
            }
            tids = now.into_iter().filter(|t| live.contains_key(t)).collect();
            last_list = Instant::now();
        }
        for tid in &tids {
            let Some(l) = live.get_mut(tid) else { continue };
            if l.h.is_null() {
                continue;
            }
            // A thread that has not run a cycle since its last sample still
            // has that stack: counted again without suspending it.
            let mut cycles = 0u64;
            // SAFETY: our handle has THREAD_QUERY_LIMITED_INFORMATION.
            let have = unsafe { QueryThreadCycleTime(l.h, &mut cycles) } != 0;
            let slack = if l.walked { APC_TAIL } else { 0 };
            if have && cycles.wrapping_sub(l.cycles) <= slack && !l.last.is_empty() {
                l.cycles = cycles;
                l.walked = false;
                cost.reused += 1;
                if let Some(n) = l.stacks.get_mut(l.last.as_slice()) {
                    *n += 1;
                }
                continue;
            }
            let s0 = Instant::now();
            // SAFETY: a thread of the target, suspended only while walked.
            unsafe {
                if SuspendThread(l.h) == u32::MAX {
                    continue;
                }
                walk(process, l.h, &mut ctx, &mut stack);
                ResumeThread(l.h);
            }
            stall += s0.elapsed();
            cost.walks += 1;
            l.last.clear();
            if !stack.is_empty() {
                match l.stacks.get_mut(stack.as_slice()) {
                    Some(n) => *n += 1,
                    None => {
                        l.stacks.insert(stack.clone(), 1);
                    }
                }
                // Read after the walk: cycles the walk's suspension caused
                // (none run in the thread's own code) don't count as running.
                l.last.extend_from_slice(&stack);
                // SAFETY: as above.
                if unsafe { QueryThreadCycleTime(l.h, &mut cycles) } != 0 {
                    l.cycles = cycles;
                    l.walked = true;
                } else {
                    l.last.clear();
                }
            }
        }
        cost.rounds += 1;
        busy += t0.elapsed();
        // Rounds on a fixed grid; a late round does not push the rest back.
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > period * 4 {
            next = now;
        }
    }
    cost.cpu_s = (own_cycles() - cycles0) as f64 / rate.max(1.0);
    cost.busy_s = busy.as_secs_f64();
    cost.stall_s = stall.as_secs_f64();
    for l in live.values() {
        if !l.h.is_null() {
            // SAFETY: our handle.
            unsafe { CloseHandle(l.h) };
        }
    }
    (live, cost)
}


fn main() {
    let all: Vec<String> = std::env::args().skip(1).collect();
    if all.first().map(String::as_str) == Some("--resolve") {
        return resolve_offline(&all[1..]);
    }
    let a = args(&all);
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
    let mods = modules(pid);
    unsafe {
        const SYMOPT_UNDNAME: u32 = 0x2;
        const SYMOPT_DEFERRED_LOADS: u32 = 0x4;
        const SYMOPT_FAIL_CRITICAL_ERRORS: u32 = 0x200;
        const SYMOPT_NO_PROMPTS: u32 = 0x80000;
        SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_FAIL_CRITICAL_ERRORS | SYMOPT_NO_PROMPTS);
        if SymInitializeW(process, std::ptr::null(), FALSE) == 0 {
            eprintln!("dbghelp could not attach (symbols will be module+offset)");
        }
        // Each module loaded by its file, so its PDB beside it is found (the
        // stack walk needs their unwind data).
        for (base, end, _, path) in &mods {
            let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
            SymLoadModuleExW(process, std::ptr::null_mut(), w.as_ptr(), std::ptr::null(), *base, (end - base) as u32, std::ptr::null(), 0);
        }
    }
    PAGES.with_borrow_mut(|c| c.images = mods.iter().map(|(b, e, _, _)| (*b, *e)).collect());
    eprintln!("sampling pid {pid} for {} s at {} Hz…", a.seconds, a.hz);
    let (live, cost) = sample(pid, process, a.seconds, a.hz);

    // Names from a session of their own with every module loaded by its file
    // at its base, not deferred (the live session left our PDB unread).
    let names_handle = 0x5678usize as HANDLE;
    let mods = modules(pid).into_iter().chain(mods).collect::<Vec<_>>();
    unsafe {
        SymSetOptions(0x2 | 0x200 | 0x80000);
        SymInitializeW(names_handle, std::ptr::null(), FALSE);
        let mut loaded = std::collections::HashSet::new();
        for (base, end, _, path) in &mods {
            if loaded.insert(*base) {
                let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
                SymLoadModuleExW(names_handle, std::ptr::null_mut(), w.as_ptr(), std::ptr::null(), *base, (end - base) as u32, std::ptr::null(), 0);
            }
        }
    }
    let syms = Symbols { process: names_handle, modules: mods.iter().map(|(b, e, n, _)| (*b, *e, n.clone())).collect() };
    let mut by_addr: HashMap<u64, u32> = HashMap::new();
    let mut by_name: HashMap<String, u32> = HashMap::new();
    let mut names: Vec<String> = Vec::new();
    let mut threads: Vec<ThreadData> = Vec::new();
    let mut tids: Vec<_> = live.into_iter().collect();
    tids.sort_by_key(|(t, _)| *t);
    for (tid, l) in tids {
        let mut merged: HashMap<Vec<u32>, u64> = HashMap::new();
        for (stack, n) in l.stacks {
            let ids: Vec<u32> = stack
                .iter()
                .map(|&p| {
                    *by_addr.entry(p).or_insert_with(|| {
                        let s = syms.resolve(p);
                        *by_name.entry(s.clone()).or_insert_with(|| {
                            names.push(s);
                            (names.len() - 1) as u32
                        })
                    })
                })
                .collect();
            *merged.entry(ids).or_default() += n;
        }
        if merged.is_empty() {
            continue;
        }
        threads.push(ThreadData { tid, group: l.group, described: l.described, stacks: merged.into_iter().collect() });
    }
    let profile = Profile {
        pid,
        seconds: a.seconds,
        hz: a.hz,
        rounds: cost.rounds,
        names,
        threads,
        sampler_cpu_s: cost.cpu_s,
        sampler_busy_s: cost.busy_s,
        stall_s: cost.stall_s,
        walks: cost.walks,
        exe: mods.first().map(|m| m.2.clone()).unwrap_or_default(),
        reused: cost.reused,
    };
    let an = analysis::analyze(&profile, &a.opts);
    let r = analysis::report(&profile, &an, &a.opts);
    print!("{r}");
    if let Some(out) = a.out {
        let _ = std::fs::write(&out, &r);
        eprintln!("written to {out}");
    }
    unsafe { CloseHandle(process) };
}
