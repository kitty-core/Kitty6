# Kitty6 development log

Work in progress, newest first. This log and the code it describes live on
the [`dev`](https://github.com/kitty-core/Kitty6/tree/dev) branch, which is
updated often. `main` only takes changes once a scripted run has shown they
don't cost frames or break the picture.

Figures come from scripted runs on one PC (RTX 4070 12 GB, 32 GB RAM, Windows 11
23H2, Series S profile, opening scene): each is the run's median over its last
45 s in the world, from the run probes.

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
