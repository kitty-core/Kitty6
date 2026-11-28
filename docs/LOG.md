# Kitty6 log (k6-fox, on foxhound)

Newest first. Measured on one PC: RTX 4070 12 GB, 32 GB RAM, Windows 11; diagnostics build, Series S profile, no
DLSS unless said otherwise. "In world" in the tables below means 2,500 draws or more a frame, as the logged F3
panel counts them. The log of the earlier, VIctory-7900XTX-based Kitty6 is on the `dev` branch.

## 2026-11-27 (evening): paired wave64 form, prepare only when the translator changes

- **Vice-Tay's paired wave64 form ported** (TaynanGT/Vice-Tay PR #208, by way of the old Kitty6 tree), on by
  default for his 29 programs plus `a10179ab7eda0dc3`. Offline (`victory translate --validate`, 32-lane host)
  all 30 translate and validate. Of the three programs foxhound refused ("no wave32 lowering"):
  `926c46bab3b86bd6` (water, submerged) and `a10179ab7eda0dc3` (water volume portals) now translate;
  `6b82bc7d22016cb5` (`CS_CollectTriangles_Meshlets`, groups of 1024) does not: its shared memory is full, it
  uses a returning LDS atomic the paired form lacks, and its 16 console waves would all have to meet at its
  barriers. victory-shader tests: 300 pass (three GPU tests failed for card memory while the game ran and pass
  alone). **Not yet checked in game**: the capture run was paused.
- **Why every build cost a 20-minute prepare**, two causes:
  1. The translator's fingerprint (`crates/victory-gpu/build.rs`) hashed all of `Cargo.lock`, so a new
     dependency anywhere (our probe's) made every translation stale. It now hashes only the lock entries
     `victory-shader` is built from (20 of 100 packages).
  2. `victory warm` puts the 88,614 known pipelines in the driver's cache, which does not keep them: 300
     pipelines warmed hours before, by the same executable, compiled again (median 44 ms, 10% over 2.7 s).
     The pipelines runs use last in the layer's own library, keyed by what they hold.
  The launcher now reads the fingerprint (`victory warm --limit 0`, one second), prepares only when it changed,
  and then warms the pipelines runs drew with (3,571) instead of all; `-FullWarm` as before.
- **Graphics check** from frames the layer saved (`VICTORY_CAPTURE` with `VICTORY_CAPTURE_FRAMES`): opening
  cutscenes (faces close up, an interior) and the first boat ride (water, sky, bridge, HUD) show no faults. Dark
  streaks on the water by the mangroves are floating debris. Every dropped draw at those places is the
  ray-tracing structure builder, which foxhound leaves off on purpose.

## 2026-11-27 (afternoon): where foxhound's frame goes

Same place for every run below: the opening cutscene the scripted Enter keys reach (`VICTORY_KEYS='28:Enter,34:Enter'`),
3,400 to 4,100 draws.

| Run | fps | draws | submit busy | "GPU busy" | note |
|---|---|---|---|---|---|
| k6-fox, our layer on (probe, panel, mods) | 24.5 | 3,687 | 32.5 ms | 37.3 ms | plain foxhound did about 23 at 3,500 draws |
| card copies 3,072 MB, windows 1,536 MB | 22.8 | 4,013 | 39.0 ms | 37.8 ms | card and shared memory exactly as before: the caps were never reached |
| `VICTORY_BARRIER_TEST=groups` (674 full-card waits a frame left out) | 22.6 | 4,067 | 36.2 ms | 38.2 ms | no change: the waits are not the cost |
| `K6_TRIM_DRIVER_CACHE_S=60` | 22.1 | 4,074 | 36.0 ms | 39.3 ms | free RAM 1.1 to 3.4 GB, frame rate unchanged |

- Our ported pieces cost nothing measurable: none of them shows in a 30-second stack sample of every thread
  (`kitty6-prof`).
- **The panel's "GPU busy" overstates the card**: it is the frame minus the stretches a clock thread sees the
  queue empty, noted when that thread wakes; with 5 to 20 submissions a frame most idle time is missed. The card
  itself (`nvidia-smi`) shows 100% busy at 95 of 215 W, memory controller about 11%, PCIe in 4 to 6 GB/s.
- Per-command card time (`VICTORY_GPU_TIMING`) is spread over about 1,150 programs, none over 1.2 ms a frame: no
  single shader to cut.
- RAM in heavy scenes: the layer's private memory 19.8 GB, about 1 GB of the PC free; 3.3 GB of it is the NVIDIA
  shader cache file mapped into the process; copies of title memory 3.5 GB; host copies of title textures 3.9 GB.
- Frame timeline (`VICTORY_TIMELINE`): on the critical chain of a frame, "a copy's refresh" (the submitting
  thread bringing its copies of title memory up to date) is the largest part, 4 to 6 ms. That and RAM are next.

## 2026-11-27 (midday): foxhound becomes the base

- foxhound (readmet3xt-lab) in the open world on this PC: about 23 fps at 3,500 draws on a first start, against
  about 12 on the old Kitty6 tree; 20 fps average over a played open-world stretch, 12 to 18 at 7,200 draws.
- Kitty6's own layer moved onto it, keeping foxhound's version wherever both do something: `K6_MAX_STEP_MS`,
  `K6_LOG_FILE`, the RAM switches, the probe, `probe-report`, the overlay, `kitty6-prof`, charts, `K6_PANEL_LOG`,
  renderer mods (`VICTORY_MODS`), `scripts\kitty6.ps1`.
