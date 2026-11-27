# Kitty6 development log

Work in progress, newest first. This log and the code it describes live on
the [`dev`](https://github.com/kitty-core/Kitty6/tree/dev) branch, which is
updated often. `main` only takes changes once a scripted run has shown they
don't cost frames or break the picture.

Figures come from scripted runs on one PC (RTX 4070 12 GB, 32 GB RAM, Windows 11
23H2, Series S profile, opening scene): each is the run's median over its last
45 s in the world, from the run probes.

## 2026-11-27 (afternoon)

- Shadow tiles (16x16 squares on faces) and water blocks: TaynanGT/Vice-Tay #208 pairs 29
  programs (18 tiled shadow mask, 4 water volumetrics, 7 sky diffusion) with float sums in
  the console's order. Their paired files replace ours wholesale; the 29 take the paired
  form by default (`VICTORY_WAVE_PAIRED_ONLY=none`: off), 29 of 29 taken offline, 0 refused.
  With the first 26 taken in the game, the user saw the face shadow tiles gone; cutscenes
  30 to 25 fps. TRANSLATOR_VERSION 6: the next run rebuilds shaders.
- Card losses (0x887A0007, no driver event) now 3 of 9 runs, each about 3 minutes in;
  same report on fxceless/victory-fork#1492 (validation finds view-type mismatches).
  Still open.
- Where RAM goes (memory map + a per-file walk): NVIDIA's own shader cache files
  (`DXCache/*.nvph`) mapped at 5 GB with 3.7 GB in our working set. `K6_TRIM_DRIVER_CACHE_S`
  (off) takes them out every period: available RAM 2.7 to 4.6 GB, but more faults early;
  to measure in a full run. Also five 1 GB private blocks (3 GB committed, likely the
  allocator's arenas) against 1.3 GB of our live allocations; to look at.
- `VICTORY_LIST_POOL=0` (halves the driver's heap on Vice-Tay#97): 6.1 fps here, rejected.
- `VICTORY_CARD_STEPS_LEARN` costs ~22 ms of card time a frame (29 off, 51 on), fps 12.6 off
  against 11.8 on.
- Vice-Tay #103 (held window copies count against the RAM budget) ported.

## 2026-11-27 (later morning)

- Footage review (15 min of play). Four kinds of fault seen: water drawn as dark
  world-space blocks; bright green/yellow blocks around the bank's lights; single
  all-white and all-black frames; a sign drawn blank. Plus vertices thrown across
  the screen and multi-second freezes between scenes.
- Same water blocks reported on Yaekai/victory-scarlett-3070#2; fxceless#1401 says the
  paired wave64 form removed them. That form is ported (`VICTORY_WAVE_PAIRED_ONLY`,
  off). On this card `=all` (878 programs) got the card removed (DXGI 0x887A0005)
  about 3 minutes in; the same build with it unset ran clean (11.6 fps). On the five
  `CS_TransmissionVolumetrics*` programs only it ran 8 min clean at the same speed
  (11.8 fps); the launcher's `-On PairedWave64` now sets those five. Whether it
  clears the water is not yet seen in the game.
- avatarmanz's decoder fixes merged (v_nop literal, s_min/s_max SCC, lane wrap,
  vertex inputs past v255, format 191) and Jrjy3's learned card steps (#434, the
  stretched characters), plus `VICTORY_TWIN_EXACT` (=1 to try on NVIDIA).
  TRANSLATOR_VERSION 4: the first run rebuilds every shader.
- `VICTORY_THIN=0` (rota's advice for NVIDIA) measured worse here: 11.6 against
  12.8 fps, prep 52 against 37 ms. `-Lean` keeps THIN=1.
- Card lost twice in six runs on this build (DXGI 0x887A0007, "reset"), each about
  3 minutes in, once with the paired form and once without; no driver timeout in the
  Windows event log. Two runs with `VICTORY_D3D12_DRED=1` did not reproduce it. The
  title's FSR4 "falling back" lines come after the loss (it rebuilds its passes), not
  before. Still open.
- Rota's settings as a set (RT_MISS=1, CARD_STEPS=1, CARD_ALL=1, DYNAMIC_SLOTS,
  PIPELINE_WORKERS=6, MEMORY_CARD_MB=1024): 11.6 fps, same as without, prep worse.
- `VICTORY_LIBRARY_PREFETCH` ported from rota (off): with the library file already
  cached, one stall over a second at world entry against 3-4, same total.
- The world-entry freeze (6-8 s) is the title's own load: 1.8 million first-touch
  faults (available RAM drops 4 GB) while its threads sit in `NtReadFile`, disk at
  9-17 MB/s, our threads mostly idle. Large pages would cut the faults 512x but need
  the lock-memory privilege and don't mix with write watching; not pursued.
- With `VICTORY_CARD_STEPS_LEARN=0` the card took 17 ms a frame against 51 with it
  (the default), in a run cut short at 3 minutes by the card loss above; to confirm.

## 2026-11-27 (morning)

- **Paired form for wave64 programs on NVIDIA** (`VICTORY_WAVE_PAIRED_ONLY=all`, off; not yet
  tried in the game; `kitty6.ps1 -On PairedWave64`). One 32-lane host wave runs a whole console
  wave: each thread carries lanes n and n + 32 and does every step for both. Two reports
  (Yaekai/victory-scarlett-3070#2, fxceless/victory-fork#1401) say forcing every wave64 compute
  program into this form took the dark patches off the water on NVIDIA at no fps cost. Ported
  from fork pull requests #1397 and #1419 (ByFxbian, after PR 854 by Pilote Production and
  darkworldhm's work); the switches follow lines9686-rgb's patch cd8fd319a. Added here: 64-bit
  float wave sums (the five `CS_TransmissionVolumetrics*` programs need them), float sums
  joined from the two halves where the console's order is not confirmed
  (`VICTORY_WAVE_PAIRS_FP32=non-ieee-rne-ftz` keeps #1419's ordered sums), more shared-memory
  atomics, and the `st64` pairs.
  - Offline, Series S shaders: 878 of 1,151 wave64 compute programs take it. The other 273
    keep their usual form: 150 would need a group barrier the paired form cannot place, 99
    have groups smaller than a console wave, 18 use 16-bit or compare-swap shared memory,
    6 other reasons. All 2,682 compute programs validate. The five
    `CS_TransmissionVolumetrics*`, the `water_micro_geom_renderer` patch passes (all but the
    small `CS_ClearWaterPatches`) and the `sr_tracing` water passes take it.
  - `VICTORY_WAVE_PAIRED_ONLY=<hash>,...` for single programs,
    `VICTORY_WAVE_PAIRED_ONLY_NOT=<hash>,...` to leave some out. Both are in the shader cache
    key when set. With `VICTORY_WAVE_WHY=1` the log says `paired lanes, first (asked for)` for
    each program.

## 2026-11-27 (early morning)

- RAM paging is the bound: fps tracks page faults across runs (17k faults/s: 17.8 fps; 42k/s: 10 fps).
  A kernel page-fault trace (xperf, 12 s) found 408,000 faults in the process: 265,000 transition
  (190,000 in `victory_safe_copy_at` reading title memory Windows had trimmed), 139,000 demand-zero
  (mimalloc purging, the probe's thread snapshot). Fixes (untested in game yet): a hard minimum working
  set (`K6_HARD_WORKING_SET`, on), `MIMALLOC_PURGE_DELAY=1000` in `-Lean`, probe thread snapshots every 30 s.
- RAM copies dropped (untested in game): `K6_WINDOW_NO_CPU_SIDE` (windows on the card without a CPU side,
  `-Lean` with 2048 MB), `K6_BUFFER_NO_CPU_SIDE` (card buffer copies filled once, after 30 s),
  `K6_SEND_SPARE_S` (idle spare send memory let go). GPU upload heaps are unsupported here (Windows 23H2).
- Ported from Peppacodes/playablegames-peppa: `VICTORY_BURY_ASIDE`, `VICTORY_LAND_ASIDE` (on in `-Lean`),
  `VICTORY_SKIP_WC_BACK`/`_KEEP`, `VICTORY_MERGE_SUBMISSIONS` (off), the placement census,
  `VICTORY_STAGING_SETTLE_S`, `VICTORY_LIST_POOL_IDLE_S` (off).
- 2/3 render scale on the Series X profile: 5.5 fps, 23,800 pages in/s. Ruled out.
- The prison-scene gradient was the scene's own lighting (not a bug). Water glitches: suspects were the five CS_TransmissionVolumetrics programs
  translated with APPROXIMATE wave64 emulation, and the water reflection passes. An exact translation is in
  progress. Skipping them by mod rule cleaned the water in shots.
- Live numbers: https://kitty-core.github.io
## 2026-11-27

- **Wave64 programs on NVIDIA, exact instead of approximate** (`K6_EXACT_WAVE64=1`,
  off; not yet tried in the game). The five `CS_TransmissionVolumetrics*` programs
  were already exact: they take the emulated console wave (`VICTORY_WAVE64_EMULATE`,
  two 32-lane waves exchanging through LDS). The `APPROXIMATE` lines seen for them
  belonged to other programs: with many workers the lines of different programs mix.
  The lines now end with the program's name and hash (`@... #hash`).
  - 35 programs really did run approximate, each half-wave counting, ORing,
    electing and reading lanes over its own 32 lanes: shadow masks and shadow-ray
    traces, `CS_ResolveVolumetrics`, scattering gathers, irradiance cache,
    fallback lighting, light clustering, temporal resolves with frame statistics,
    lens-flare scans, the light atlas, fire and decal resolves. With the switch,
    34 of them take the emulated console wave first. All validate and build on
    the 4070; no other program changes. `CS_ProjDecalResolve_Scalarized` stays
    approximate.
  - `K6_EXACT_WAVE64=2` also comes before the per-half allocation idiom, whose
    lists come out in a different order (the water reflection pre-passes
    `CS_RayPrePass_Water_`, `_SSR_`, `CS_Tiling_Generate_Water_`, tiled cascade
    shadows, instance culling): 89 programs change, all validate and build. For
    checking only.
  - `VICTORY_WAVE64_EMULATE` is now in the shader cache key. Before, turning it
    off still got the emulated translations from the cache.

## 2026-11-26 (later that night)

- **PC controls** (on `dev`, not yet tried in the game): mouse camera,
  mouse-button triggers, Shift sprint, Space jump, F interact, R reload, Q
  cover, Tab wheel, Ctrl crouch (README, "Controls"). `K6_CONTROLS=pad` for
  the old layout.
- **RAM:** the game asks Windows to keep half the PC's RAM of it resident (a
  soft minimum working set, `K6_MIN_WORKING_SET_MB`), so the trimmer takes from
  idle programs first. Being tested.
- **Focus matters:** the game runs noticeably faster with its window in front
  (Windows favours that window for time slices, the GPU and memory). Probes
  now record it (`focused` column); earlier scripted runs were unfocused.
- **Where the 22 GB goes:** ~5.3 GB copies of title memory, ~4.1 GB grown
  while the 1,501 host command lists were made (being cut), ~2.2 GB textures,
  ~1.3 GB our heap; the rest is the title's own memory.
- [docs/probe-runs.md](probe-runs.md): every saved run and what it showed.

## 2026-11-26 (night)

**Blocky shadows and faces: fixed.** The stair-stepped sun-shadow edges, square
cut-outs in sunlit patches and blocky tiles on faces came from the 16-bit path
for halves (half views and integer format loads, ported earlier today). With
both off they are gone in the game. The launcher now sets
`VICTORY_HALF_VIEWS=0` and `K6_HALF_LOADS=0` by default; they cost some frame
rate back, to be won again another way.

- **Renderer mod:** the 10.9 ms single-group compute pass `02384a60fda6f2d0`
  now runs every 4th frame (`mods/kitty6.k6mod`). The card's wait fell from
  ~29 to ~11 ms a frame in the opening; the frame is now bound by the CPU
  preparing it (~30-35 ms). No visible change in a skip-vs-run comparison.
- **Stale textures** (after fxceless e4245f3c1b): textures under a finished
  plain read are uploaded again, and dynamic descriptor slots go back only to
  the heap that gave them. Textures painted through another view of their
  pages are fingerprinted whole and kept checked (`K6_FULL_PRINT_KB`,
  `K6_ALIASED_RECHECK_EVERY`). No measurable cost (19.4 fps).
- **Stutters are paging, not pipelines:** in a 9-minute play run only 21
  pipelines were made, while every 0.6-1.3 s stall lined up with tens to
  hundreds of thousands of page faults. The process holds ~21.5 GB private
  with ~2 GB of RAM free. Memory is the next target.
- **Ray tracing on (`-RayTracing`)** crashed in the title after ~2 minutes and
  costs ~40 ms of card time; the sun shining through walls (shadow rays with no
  structures to hit) needs another way, e.g. only unpacking the shipped static
  structures.

## 2026-11-26 (late)

**The "regression" wasn't one.** The build from before today's ports, run
again under the same conditions, gets the same 16.7 fps with 25.2 ms of GPU
wait as the current code. The ~20 fps runs were taken under different
conditions. Today's slower runs had other work on the card (shader tests) and
the PC locked. From now on every comparison runs a reference build in the same
session.

- Ruled out as causes: 4 KiB texture placements, the saveexec and fmac fixes,
  the 16-bit halves commits, the light stand-in's strength.
- Merged on `dev`:
  - mimalloc allocator (trustno1 PR #11)
  - joined compares of neighbouring copy ranges (VI-BR-PORT 2f59f1dfd8,
    `VICTORY_CARD_COMPARE_JOIN`)
  - an off switch for half views (klbf-lab PR #113, `VICTORY_HALF_VIEWS=0`)
    and for integer format loads (`K6_HALF_LOADS=0`)
  - an `-Rtx4070` launcher preset (after Vice-Tay PR #69)
- Release builds compile again (the renderer mods used a diagnostics-only
  counter).
- New Kitty6 switches are named `K6_...` from now on.
- **Next:** a GPU timing run to rank shaders by milliseconds, then cut the
  costly passes; upstream's port-2 fallback budget (4aeb6d08, reported 15-25
  ms a frame); render scale.

## 2026-11-26 (evening)

**Where it stands:** about 17 fps on `dev`, down from about 20 before two port
batches went in. Card memory is down from 7.4 GB to 5.4 GB.

- **Regression being chased.** After the first port batch, the card's time per
  frame doubled (13.7 ms to about 26 ms) while preparation stayed the same.
  Tested so far:
  - 4 KiB small-texture placements: not the cause (16.8 fps with them off). They
    save about 2 GB of card memory, so they stay.
  - Saveexec scan and fmac fixes reverted: not the cause (16.1 fps).
  - The four 16-bit "halves" translation commits reverted: being tested now.
    klbf-lab PR #113's `VICTORY_HALF_VIEWS=0` is being ported as an off switch
    for them.
- **Merged on `dev`, not yet run in the game:**
  - Purple patches: magenta values left in the probe memory are no longer used
    as light (after Fayberr/Victory-United 1bf62e10, `VICTORY_AMBIENT_MAGENTA`,
    on).
  - NaN and infinity scrub for float image and buffer stores (fxceless PR #837,
    `VICTORY_NAN_SCRUB=1`, opt-in).
  - Full barrier waits for shadow-tile programs when barriers are thinned (after
    voidopcode/vilocity, `VICTORY_GROUP_SYNC_FULL`). It does nothing unless
    `VICTORY_GROUP_SYNC` is set, so it isn't what causes our blocky tiles.
- **Being ported:** mimalloc allocator (trustno1 PR #11), merged compares of
  neighbouring copy ranges (VI-BR-PORT 2f59f1dfd8), an RTX 4070 preset (Vice-Tay
  PR #69).
- **Next:** a GPU timing run (`VICTORY_GPU_TIMING=1`) to rank the title's shaders
  by milliseconds a frame. That ranking picks which draws and passes to cut with
  Kitty6 renderer mods (`mods/*.k6mod`). After that, render scale 0.75 and 0.5.

## 2026-11-26 (afternoon)

- **First port batch** (VI-BR-PORT, Vice-Tay): 16-bit values kept intact through
  half views (for mouths), 4 KiB small-texture placements on non-AMD cards,
  ray-miss pass skips (opt-in), BC7 rotation, fmac and saveexec shader fixes.
- **Memory batch:**
  - copy-back buffers reused (fxceless PR #1359)
  - texture copy-backs written on their own thread (trustno1 PR #7)
  - idle textures freed under memory pressure (Fayberr)
  - the idle snapshot pool freed (Vice-Tay PR #41)
  - unchanged pages skipped (VIniVIdiVIci, opt-in `VICTORY_WINDOW_SKIP_SAME`)
- **Renderer mods** (`mods/*.k6mod`), the in-game dev menu (F4), charts in the
  README.

## 2026-11-26 (morning)

- About 20.5 fps standing in the opening scene, from:
  - buffer copies on the card (4 to 16 fps)
  - our lock fix (4 to 19 fps)
  - upstream NVIDIA descriptor work (PR #237)
  - readback tracking (PR #150)
