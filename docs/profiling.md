# Profiling: where the threads' time goes

The probes ([probes.md](probes.md)) say *that* a run is short of something;
`tools/kitty6-prof` says *where* each thread spends its time. It samples every
thread's call stack of the running `victory.exe`, names frames from the PDB
beside the exe, and reports per thread and per role what runs and what waits,
on what and from where.

It is its own cargo workspace:

```powershell
cd tools\kitty6-prof
cargo build --release --offline
target\release\kitty6-prof.exe --help
```

## A run

```powershell
kitty6-prof --seconds 20 --hz 50 --out prof.txt --json prof.json --folded prof.folded
```

| Option | What |
|---|---|
| `--pid N`, `--name EXE` | the process (default the first `victory.exe`) |
| `--seconds S`, `--hz H` | how long and how often (each thread `H` times a second) |
| `--top N` | rows in the all-thread tables |
| `--out FILE` | the text report, also printed |
| `--focus NAME` | what runs under the frames whose name contains `NAME` |
| `--json FILE` | the numbers behind the report, for `diff` |
| `--folded FILE` | stacks in the folded format, for flame graphs |
| `--fence PAT` | a frame-name pattern (repeatable) that marks a kernel event wait as a card fence wait |

The report, in order:

- **sampler**: its own CPU (thread cycle counts), how many stacks were walked,
  how many samples were reused (the thread had not run a cycle since its last
  walk, so it was not suspended again) and how long a walked thread stays
  suspended.
- **threads**: per thread description (numbered workers together, `x8`),
  running %, waiting %, and waiting split by kind.
- **roles**: threads grouped by what they are for. With a description:
  `victory-submit` is `submission`, `victory-list-#` are `list recorders`,
  numbered workers and the wide/land/under/pipelines pools are `pool: <name>`,
  others go by name. Without one, by the entry function: the CRT's main thread
  is `main`, title code (frames outside the exe and the system/driver modules)
  `title threads`, `rayon_core` is `pool: rayon`, the card driver's threads
  `driver (<module>)`, a system thread pool goes by what its callbacks run.
  Per role: busy % and ms/s, waiting by kind, top self functions of busy
  samples, top inclusive functions, waits by site.
- **waits by site**, over all threads and per thread: a sample waits when the
  system frames at its top (ntdll, kernelbase, the D3D12 runtime and driver,
  Rust's std/parking_lot sync code) hold a blocking call (`NtWaitFor*`,
  `NtDelayExecution`, `SleepConditionVariable*`, `RtlAcquireSRWLock*`,
  `RtlpWaitOnCriticalSection`, `WaitOnAddress`, park). The site is the first
  frame of ours above them; the kind is `lock`, `event` (condition variables,
  parking, kernel events), `fence` (a wait inside the D3D12 runtime or driver,
  or a kernel event waited for in `victory_gpu::host`/`queue` or a
  `*fence*`/`*settle*` function), `sleep`, `idle` (pool workers waiting for
  work) or `message`.
- per thread description: what it runs, waits by site, call chains, inclusive.

Times are **thread-ms per second** of wall time: samples / rounds × 1000. One
thread waiting the whole time is 1000 ms/s; 8 list threads queued 75% of the
time on one lock are 6000 ms/s at that site. Percentages are of the thread's
(or role's) samples.

## Cost

Stacks are kept as raw addresses and named once at the end; target memory is
read a page at a time and cached; threads that have not run since their last
walk are not suspended again. Measured on `examples/target.rs` (14 threads:
lock holders and queuers, a condition variable, sleepers): 2.9% of one core at
50 Hz, 4.5% at 200 Hz; a walked thread is suspended ~40 µs. The game has many
more threads, so read the `sampler:` line of each report; 50 Hz for 20 s is
enough for the shares that matter.

## Flame graphs

```powershell
python tools\charts\flame.py prof.folded prof.svg --title "S-tl, 50 Hz"
python tools\charts\flame.py prof.folded prof.svg --color role --per-role flames
```

A standalone SVG: width is the share of samples, root at the bottom, a tooltip
per frame. Colors by module (our crates warm, system blue-grey, D3D12/driver
green, title code violet) or by role; the last `[wait: kind]` frame of a
waiting stack has its own muted color (`--no-waits` drops it). `--per-role`
writes one SVG per role.

## A/B: two builds or a switch off and on

```powershell
# a: the build or switch as it was
scripts\play.ps1 -Exe <a.exe> -Lean -Probe
kitty6-prof --seconds 20 --hz 50 --json a.json --folded a.folded --out a.txt
# b: the change
scripts\play.ps1 -Exe <b.exe> -Lean -Probe -More @{ VICTORY_SOMETHING = '1' }
kitty6-prof --seconds 20 --hz 50 --json b.json --folded b.folded --out b.txt

kitty6-prof diff a.json b.json --top 10
victory probe-report <a.csv> <b.csv>
```

Sample the same stretch of the game in both (e.g. the opening scene, once the
world is in). `diff` shows, for all threads and then per role: busy and
waiting time by kind (ms/s, then % → %), and the wait sites, busy-self and
inclusive functions whose time changed most, in ms/s and percentage points of
the role's samples. A lock that went away shows as a `lock` site falling by
hundreds of ms/s and the role's busy time rising; a change that only moves a
wait shows as one site falling and another rising.

If the exe was renamed aside and rebuilt after the run, name the bare
`victory.exe+0x...` frames with `kitty6-prof --resolve <the exe that ran>
0xOFFSET...`.

To try the tool without the game: `cargo run --release --offline --example
target` in `tools\kitty6-prof`, then `kitty6-prof --name target.exe`.
