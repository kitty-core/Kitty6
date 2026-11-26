# Kitty6 development log

Work in progress, newest first. This log and the code it describes live on
the [`dev`](https://github.com/kitty-core/Kitty6/tree/dev) branch, which is
updated often. `main` only takes changes once a scripted run has shown they
don't cost frames or break the picture.

Figures come from scripted runs on one PC (RTX 4070 12 GB, 32 GB RAM, Windows 11
23H2, Series S profile, opening scene): each is the run's median over its last
45 s in the world, from the run probes.

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
