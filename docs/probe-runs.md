# Probe runs: what each one showed

The runs below are probe files (`VICTORY_PROBE`, see [probes.md](probes.md))
saved under a name on the test PC (RTX 4070 12 GB, 32 GB RAM, Windows 11 23H2,
Series S profile, the opening scene unless it says "played"). Each figure is the
run's median over its last 45 s in the world (seconds with 500 draws a frame or
more). The table comes from `python tools/charts/probe_table.py`; the CSV files
stay on the test PC (`%LOCALAPPDATA%\victory\probes`).

How to read it:

- `prep_ms` is the submitting thread preparing a frame (CPU); `gpu_ms` is that
  thread waiting for the card. Whichever is larger bounds the frame.
- `private_mb`, `avail_ram_mb` and `sys_pages_in_per_s` are the RAM story. On a
  32 GB PC, more than ~1,500 pages read back a second means the frame is
  paging; free RAM under ~2 GB comes with stalls.
- `focused` (from 2026-11-26 night on) is the share of seconds the game's window
  was in front. Windows favours the window in front (time slices, the GPU
  scheduler, memory), so runs taken unfocused understate the frame rate.
  Compare runs only with the same focus, and run a reference build in the same
  session.
- The early runs of 2026-11-26 (A to L) were on a cold pipeline cache; their low
  frame rates are pipelines and paging together, not comparable with later runs.

## Lessons that hold

1. **Bytes between the CPU and the card** were the first wall: copies kept on
   the card took the opening from 4 to 16 fps (F-mix4).
2. **One lock on the serial path** cost two thirds of the frame (M-prof1 ->
   N-fix1).
3. **The card was then bound by a few heavy compute passes**, not by draw
   count: one single-group pass cost 10.9 ms a frame; running it every 4th frame
   (renderer mod) took the card's wait from ~29 to ~11 ms (M1-skip0238, tiles1).
4. **Now the wall is RAM**: ~22 GB private, 2-3 GB free, and every long stall is
   page faults (play-every4b, memmap). The breakdown (memmap): ~5.3 GB host
   copies of title memory (windows), ~4.1 GB that grew while the 1,501 host
   command lists were made, ~2.2 GB textures and their pool, ~1.3 GB our own
   heap; the title's own memory is most of the rest.
5. **Compare in the same session against a reference build** (O1-oldbuild):
   the same code ran 16.7 to 20 fps on the same day under different conditions.
6. **Picture bugs from precision**: the 16-bit path for halves made sun shadows
   stair-stepped and faces blocky on NVIDIA (memmap); the float path is the
   default.

## Runs

| run | saved | world s | fps | ms_p95 | prep_ms | gpu_ms | private_mb | avail_ram_mb | sys_pages_in_per_s | vram_used_mb | focused | what it showed |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| A-base | 11-26 03:49 | 27 | 4.6 | 419.8 | 75.2 | 48.6 | 26012.5 | 1198.0 | 6799.0 | 9774.5 |  | first lean baseline, cold pipeline cache: paging and pipelines both |
| B-lean | 11-26 03:55 | 46 | 4.1 | 367.4 | 79.9 | 47.3 | 24147.0 | 2887.0 | 1623.0 | 10925.0 |  | -Lean memory settings |
| C-mix1 | 11-26 04:09 | 32 | 3.9 | 364.6 | 41.0 | 73.0 | 19782.0 | 1772.5 | 3689.5 | 7488.0 |  |  |
| D-mix2 | 11-26 04:14 | 31 | 3.7 | 383.5 | 73.1 | 70.7 | 19851.5 | 3443.0 | 5029.5 | 7511.5 |  |  |
| E-mix3 | 11-26 04:21 | 19 | 4.0 | 379.6 | 79.3 | 103.3 | 19169.5 | 2165.5 | 2566.5 | 6774.0 |  |  |
| F-mix4 | 11-26 04:28 | 18 | 16.0 | 80.8 | 38.6 | 1.8 | 19986.0 | 1754.0 | 1382.0 | 7483.0 |  | **buffer and window copies on the card** (VICTORY_CARD_BUFFERS_MB=1536, VICTORY_CARD_WINDOWS_MB=1024): 4 -> 16 fps, the biggest single win |
| G-fast1 | 11-26 04:41 | 16 | 11.2 | 168.9 | 80.4 | 0.0 | 19321.0 | 2244.0 | 11700.0 | 7388.5 |  | 3 GB caps for both: worse (2.5 GB more private, paging) |
| H-merged1 | 11-26 09:47 | 15 | 3.8 | 467.4 | 124.5 | 0.0 | 20181.5 | 1037.0 | 10656.5 | 7600.5 |  |  |
| I-portsA | 11-26 09:50 | 13 | 3.3 | 566.6 | 243.9 | 0.0 | 19834.5 | 3681.0 | 6313.0 | 7445.0 |  |  |
| J-portsB | 11-26 09:53 | 13 | 2.8 | 352.0 | 126.4 | 0.0 | 20028.0 | 2409.0 | 5206.0 | 7376.0 |  |  |
| K-premerge | 11-26 09:56 | 18 | 3.9 | 419.5 | 49.1 | 8.4 | 20118.5 | 3853.5 | 1688.5 | 7626.0 |  |  |
| L-xma0 | 11-26 10:01 | 18 | 3.8 | 504.3 | 73.1 | 0.0 | 20290.0 | 2855.0 | 1372.0 | 7454.0 |  |  |
| M-prof1 | 11-26 10:09 | 144 | 6.9 | 195.1 | 23.4 | 0.0 | 20435.0 | 3979.5 | 53.5 | 7567.0 |  | before the EXACT lock fix: 8 recording threads ~75% queued on one lock |
| N-fix1 | 11-26 10:18 | 154 | 19.0 | 74.3 | 26.0 | 21.1 | 20291.5 | 3728.0 | 24.5 | 7416.0 |  | **shadow::exact buffers reused outside the lock** (VICTORY_EXACT_REUSE): 6.9 -> 19 fps |
| O-why | 11-26 10:23 | 156 | 19.8 | 67.8 | 25.3 | 19.6 | 20319.0 | 3881.0 | 17.5 | 7439.0 |  | baseline after the lock fix, used for the where-the-frame-goes analysis |
| P-fill2 | 11-26 10:27 | 166 | 19.3 | 71.5 | 25.4 | 20.7 | 20246.0 | 4245.0 | 17.5 | 7444.0 |  | VICTORY_FILL_STALE_BYTES: the settle wait moves on, ~+1 fps only |
| Q-stale | 11-26 10:33 | 171 | 19.1 | 63.2 | 27.5 | 19.8 | 20446.0 | 3833.0 | 12.5 | 7516.0 |  |  |
| R-stale1m | 11-26 10:37 | 171 | 19.9 | 58.7 | 29.4 | 13.6 | 20443.0 | 3708.5 | 7147.0 | 7425.0 |  | a paging spike mid-run (7,147 pages in/s) at the same fps |
| S-tl | 11-26 10:44 | 168 | 19.2 | 70.7 | 27.8 | 18.6 | 20537.0 | 3624.0 | 17.5 | 7378.5 |  | VICTORY_TIMELINE capture (critical chain per frame) |
| U-overlap | 11-26 10:57 | 168 | 19.7 | 58.4 | 29.3 | 14.3 | 20513.5 | 3546.0 | 17.0 | 7515.0 |  |  |
| V-leaveout | 11-26 11:04 | 156 | 5.5 | 186.1 | 23.3 | 156.3 | 20299.0 | 3807.0 | 7.0 | 7543.0 |  | a leave-out test that cost the frame (5.5 fps): not kept |
| W-merge | 11-26 11:10 | 162 | 19.2 | 70.9 | 26.0 | 20.8 | 20285.0 | 3880.5 | 32.0 | 7549.0 |  |  |
| X-trace | 11-26 11:15 | 138 | 19.6 | 68.2 | 25.8 | 19.9 | 20577.0 | 3619.0 | 11.0 | 7501.0 |  |  |
| Y-changed | 11-26 11:21 | 161 | 19.3 | 68.8 | 26.6 | 19.3 | 20526.5 | 3872.5 | 16.0 | 7528.0 |  |  |
| Z-nopace | 11-26 11:26 | 163 | 19.7 | 68.0 | 26.7 | 18.4 | 20654.5 | 3480.5 | 16.0 | 7548.0 |  |  |
| D-drive | 11-26 11:31 | 172 | 18.4 | 78.2 | 31.0 | 14.5 | 20271.0 | 3733.5 | 244.0 | 7526.5 |  |  |
| D2-pr237 | 11-26 11:36 | 163 | 20.4 | 69.2 | 29.7 | 13.7 | 20392.0 | 4000.5 | 133.5 | 7524.5 |  |  |
| E-bar | 11-26 11:41 | 182 | 18.0 | 81.6 | 33.8 | 7.7 | 20696.0 | 3281.0 | 827.0 | 7639.0 |  |  |
| G-bar | 11-26 11:48 | 185 | 19.0 | 72.8 | 28.4 | 11.2 | 20836.5 | 3262.0 | 248.5 | 7601.0 |  |  |
| H-seriesx | 11-26 12:08 | 146 | 15.2 | 101.9 | 28.6 | 25.8 | 24821.0 | 2192.0 | 3308.5 | 8008.5 |  | Series X profile: 4.5 GB more private, more paging, 15 fps; Series S stays |
| P1-ports | 11-26 13:29 | 180 | 17.0 | 81.8 | 27.0 | 26.2 | 19923.0 | 3734.5 | 23.0 | 5268.0 |  |  |
| P2-both | 11-26 14:01 | 190 | 17.1 | 81.0 | 26.6 | 25.4 | 19456.0 | 3442.0 | 8.0 | 5507.0 |  |  |
| P3-pool64 | 11-26 14:06 | 198 | 16.8 | 82.3 | 26.9 | 26.7 | 21828.5 | 3619.5 | 25.0 | 7439.5 |  | pool of 64: 2.4 GB more private and card memory, no gain |
| B-bisect-a | 11-26 16:50 | 170 | 16.1 | 91.6 | 30.1 | 22.6 | 19415.0 | 2391.5 | 236.5 | 5138.0 |  |  |
| B-bisect-b | 11-26 16:59 | 191 | 16.5 | 85.9 | 29.6 | 24.3 | 19848.5 | 2148.5 | 96.5 | 5273.0 |  |  |
| A1-amb08 | 11-26 17:09 | 178 | 16.9 | 83.1 | 28.6 | 23.7 | 19437.0 | 1497.0 | 262.0 | 5234.0 |  | VICTORY_AMBIENT 0.8 vs 1: no fps difference |
| O1-oldbuild | 11-26 17:16 | 193 | 16.7 | 81.3 | 28.9 | 25.2 | 21951.0 | 3751.5 | 31.0 | 7489.0 |  | **reference build** (pre-port 95bec4c7) in the same conditions: 16.7 fps, so the afternoon's "regression" was the test conditions |
| M1-skip0238 | 11-26 17:29 | 231 | 18.9 | 75.0 | 26.9 | 20.4 | 21344.5 | 2627.5 | 24.0 | 5421.5 |  | in-run A/B of skipping compute 02384a60 (renderer mod): ~+3 fps, card wait 29 -> 19-21 ms |
| play-every4b | 11-26 17:46 | 422 | 11.8 | 128.0 | 58.8 | 0.1 | 22265.0 | 3682.0 | 5078.0 | 5675.0 |  | **a person playing** (every-4 mod): stalls of 0.6-1.3 s line up with page faults; only 21 pipelines made in 9 minutes |
| tiles1 | 11-26 17:58 | 207 | 20.7 | 71.9 | 29.6 | 10.9 | 21831.5 | 3709.0 | 341.0 | 5629.0 |  | stale-texture fixes (after fxceless e4245f3c1b) and the every-4 mod: 20.7 fps, card wait 11 ms, no cost |
| memmap | 11-26 18:05 | 394 | 3.3 | 438.3 | 230.5 | 0.0 | 24505.0 | 2293.0 | 9835.0 | 5771.0 |  | halves by the float path + memory map every 30 s (played): blocky shadows gone; the end is a paging collapse (9,800 pages in/s) |
