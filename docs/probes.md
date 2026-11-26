# Probes: what a slow PC is short of

The measurements in [findings.md](findings.md) are of one PC with 24 GB on the
card and 64 GB of RAM. On a smaller PC the wall can be another one entirely:
the system paging the title's memory to disk, the card's memory full, host
pipelines still being made. The probes measure those over a whole run, a row a
second, so that two runs (a switch on and off, one PC and another) can be laid
side by side.

Diagnostics builds only (`cargo build --profile diag -p victory --features diagnostics`).

```powershell
scripts\play.ps1 -Exe <your copy's executable> -Probe       # or VICTORY_PROBE=<folder>
target\diag\victory.exe probe-report <probe file>            # what it says
target\diag\victory.exe probe-report <before> <after>        # the second against the first
```

Where the threads' time goes within a run (waits by site, thread roles,
an A/B diff of two builds, flame graphs): [profiling.md](profiling.md).

The folder must be outside the source tree. `VICTORY_PROBE_STALL_MS` (100) sets
the frame time from which a frame is a stall.

## What a row holds

| Column | What |
|---|---|
| `t_s`, `frames`, `fps` | seconds since the probes began; frames and rate of the row |
| `ms_mean`, `ms_p50`, `ms_p95`, `ms_max` | the spread of the row's frame times (present to present) |
| `over33`, `over100`, `over500` | frames that missed 33.4, 100 and 500 ms |
| `stall_ms`, `stall_faults`, `stall_pipelines_made` | the slowest frame at or over the stall time, with the page faults the process took and the host pipelines made during it |
| `prep_ms`, `gpu_ms`, `draws`, `dropped` | the frame line's: ms preparing submissions and on the GPU per frame, draws, draws dropped without a host pipeline |
| `pipelines_waiting`, `pipelines_made`, `pipeline_ms_mean`, `pipeline_ms_max`, `pipeline_wait_ms` | host pipelines queued and not made; made in the row, their mean and longest making time; ms first uses waited for one |
| `ws_mb`, `private_mb`, `faults_per_s` | the process's working set (in RAM), private bytes (committed), page faults a second (soft and hard) |
| `io_read_mb_s` | bytes the process read from files (the title's streaming; paging is not counted) |
| `sys_pages_in_per_s` | `\Memory\Pages Input/sec`: pages the system read back from disk for every process, the measure of paging |
| `avail_ram_mb`, `avail_commit_mb`, `mem_load_pct` | free RAM, free commit (RAM and page file), the system's memory load |
| `vram_budget_mb`, `vram_used_mb`, `vram_shared_mb` | the system's budget of the card's memory for the process, its use, and the PC's memory the card uses for it |
| `cpu_cores` | the cores the process kept busy |
| `busiest_thread`, `busiest_thread_cores` | the busiest thread of the last 5 s and its cores (near 1.0: a serial wall) |

The log gets two more kinds of line (target `victory_gpu::probe`): every 5 s
the ten busiest threads and the busiest kinds of thread (a pool's threads added
up), and for each second with a stall what happened during its slowest frame.

## The live overlay

`kitty6-overlay` (`apps/kitty6-overlay`) draws the same numbers live over the
game: two click-through strips down the left and right edges of the game's
window, the centre left clear.

```powershell
cargo build --profile diag -p kitty6-overlay
scripts\play.ps1 -Exe <your copy's executable> -Lean -Overlay   # starts it with the game, stops it after
target\diag\kitty6-overlay.exe --demo                          # a stand-in window with made-up numbers
target\diag\kitty6-overlay.exe --demo --shot <file.png>        # the demo drawn once off screen
```

- **F6** hides and shows it, and turns the game's sampling off and on with it:
  hidden, the game only checks a flag at each present. **Ctrl+Alt+F6** quits.
  (If another program holds F6 the key is watched instead; the hint shows `HIDE*`.)
- Left: frame times of the last 512 presents (guides at 33, 50 and 100 ms,
  coloured by how late), fps and the second's p50/p95/max; ms preparing against
  ms on the GPU per second; draws and dropped draws; host pipelines (waiting,
  made a second, ms each, waits, refused); the likeliest bottleneck over the last
  30 s by the same rules as `probe-report` (`victory_core::verdict`).
- Right: process private bytes and working set against RAM, system RAM and
  commit, system paging (red over 1,500 pages a second); the card's budget, use
  and shared memory; disk reads; the busiest threads and kinds of thread; the
  stall log (frames over the stall time with the faults and pipelines during
  them); the layer's holdings (host copies, host textures, upload buffers,
  staging, scratch, texture uploads a second, allocators renewed, left-out
  levels, the title's frame) and the `VICTORY_*` switches set.

How it is fed: every diagnostics build publishes a named shared block,
`Local\Kitty6Telemetry` (layout and seqlocks: `victory_core::telemetry`; writer:
`victory_gpu`'s `telemetry` module), whether or not `VICTORY_PROBE` is set. The
overlay sets a tracking flag and a heartbeat in it; while both are fresh the
probes' sampling runs (each present adds its time and page faults to a ring, and
the probe row each second goes into the block), and stops within 3 s of the
overlay hiding or quitting. A present never waits on the block: a write that
finds another under way is skipped. The overlay draws with Direct2D's software
rasterizer into a layered window, at about 30 Hz: no card memory, no GPU time.
The probe row is now written every second (it had come every 5 s, with the
frame spread of those 5 s against the rate of the last one).
## The report

`probe-report` prints the whole run and, apart, the seconds in the world
(`--world-draws`, 500 draws a frame: the menus draw a few dozen). For each: the
spread of frame times, where a frame went, pipelines, RAM, the card's memory,
the processor; which measures the frame time moved with (correlation over the
seconds); and the likeliest causes, first the likeliest, with the evidence and
what to try:

- **RAM**: a quarter of the seconds or more paging hard (1,500+ pages in a
  second), or free RAM under 1 GB in 40% of them.
- **Card memory**: in use at 95% or more of the budget in 30% of the seconds, or
  more than 1 GB of it in the PC's memory.
- **Pipelines**: still being made in 30% of the seconds, with draws dropped or
  first uses waiting. A run's first minutes; `victory warm --used` makes them
  ahead.
- **Preparing submissions** or **the GPU**: over 70% of the median frame.

With two files, the world's numbers of the second against the first.

## First run on a smaller PC (2026-11-26)

i7-14700KF, RTX 4070 12 GB, 32 GB RAM (20 GB page file), Series X profile, the
first run of a fresh cache, from the frame lines and the system's counters
(before the probes existed): in the opening cutscene 0.3 to 1.8 fps (570 to
3,700 ms a frame). At the same time the system read some 10,700 pages a second
back from disk, free RAM was 1.7 GB, and the process held 22.5 GB private with
5 GB of it in RAM; 850 to 890 host pipelines were waiting and 1,500 to 2,900
draws a frame were dropped for want of one. Both walls at once: paging (this
PC has half the RAM the fork was measured with) and the first run's pipelines.

## What `-Lean` changes for memory

| Switch | Effect |
|---|---|
| `VICTORY_TEXTURE_MIP_BIAS=1` | Host copies of 2D textures of 1024 texels or more on a side (`VICTORY_TEXTURE_MIP_BIAS_MIN`) begin one level lower: about three quarters fewer bytes each, softer up close. Needs held levels, so `-Lean` no longer sets `VICTORY_LEVELS_ON_DEMAND` |
| `VICTORY_TEXTURE_ROOM_PERCENT=80` | The card's texture pass stops at 80 % of the budget, not 90 % |
| `VICTORY_ALLOCATOR_RENEW` | Every 240th reset a host list's command allocator is made anew instead of reset, so one large frame does not hold its memory for the rest of the run (default on a PC of 32 GB or less with `VICTORY_RESIDENCY=1`; `0` turns it off) |
| kept send memory | 256 MB instead of 768 MB on the same PCs |
| `VICTORY_D3D12_DRED` | not set: in a diagnostics build it also keeps a book of up to 400,000 released resources |

`VICTORY_INPLACE` needs `OpenExistingHeapFromAddress1`; where the driver
answers `0x80004002` (no such interface) it falls back to copies, as before.
With late wait on (`VICTORY_RELAY=2`, the launcher's default), `=2` places only
windows outside buffers.
