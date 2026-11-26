# Scouting and fixing: the standing routine

Read this at the start of every session and come back to it often. The work
isn't only "measure, change, measure". It is also keeping up with what the
other forks find and attacking each problem from more than one side.

## 1. Scout often (every session, and whenever stuck)

- **The tracker**: https://vuyp.github.io/gta6-pc-race/ (data in
  `vuyp/gta6-pc-race` `src/data.json`). Look at who moved up, and why.
- **Pull requests and recent commits** on the other forks, open *and* merged:
  `gh pr list -R <owner/repo> --state all --limit 30`, `gh api repos/<o>/<r>/commits`
  (on this PC: `~/tools/gh/ghk`, gh with git's GitHub login).
- **PortWatch** (https://portwatch-vi.xyz/data/tracker.json): outside picture
  scores the tracker relays. On 26 Nov it rated DexNom/VIctory best (real ray
  tracing on AMD, about 1 fps), then Belobug.
  Start with upstream avatarmanz/VIctory-7900XTX, then Vice-Tay, VI-BR-PORT,
  klbf-lab, VIctory-NVIDIA, VIctory-RDNA, victory-fork, Kelvaroq and AnyXboxX, then
  any new names on the tracker.
- **What to look for, in this order:**
  1. Memory: copies per frame, readback, paging, card memory, pools, placements.
  2. Picture bugs we have: purple noisy patches, orange glow on clothing,
     grey-blue arms, faces, mouths, stale light.
  3. Shaders NVIDIA refuses (wave64), and anything else marked wave32 or NVIDIA.
  4. Frame-time wins that need no picture trade.
- **Which repo has the best picture right now?** The user knows of one with
  near-perfect graphics. Find it, compare its footage with ours, and port what
  makes the difference (switches, shader fixes, format fixes).
- Write each find, tried or not, in the log below: repo, PR or commit, what it
  claims, and what it did here.

## 2. Think outside the box

- **Come at every problem from at least two sides** before you call it hard. For
  a slow copy: copy less, copy later, copy on the card, compress it, skip it
  when unchanged, or make the shader stop writing it. For a picture bug: shader
  translation, texture format, missing resource, stale data, wrong barrier, or
  the wave size.
- **Borrow from elsewhere**: emulators (Xenia, RPCS3, Ryujinx, yuzu, Dolphin),
  Proton/VKD3D, DXVK, driver tricks. Someone has usually solved the same class
  of problem.
- **Change the question.** If the card can't map memory, can the CPU avoid
  needing it? If a pass is slow, does the frame need it every frame? (That's
  what the Kitty6 renderer mods in `mods/*.k6mod` are for.)
- **Write our own tools when the existing ones stop helping** (profiler, dev
  menu, charts, replay). A tool that shows the problem is half the fix.
- **Skip what isn't needed.** Don't polish what doesn't move frame rate or the
  picture. Drop a dead end quickly and say so in the log. Park any idea that
  needs a Windows or driver update the PC doesn't have (GPU upload heaps need
  24H2).
- **Keep it safe.** Put anything risky behind a `VICTORY_...` switch. Never
  ship a speed-up that breaks the picture without saying so.

## 3. Prove it

- A/B in scripted runs (`scripts\play.ps1 -Lean -Probe`, the dev menu's MODS
  item for in-run A/B), then footage (`-Dev`) against the console trailer
  shots. Big, consistent differences only.
- Update the README's figures and charts (`tools/charts/charts.py`), then push.
  Never push title files, and never let the Windows username into the repo.

## Log

| Date | Source | What | Result here |
|---|---|---|---|
| 2026-11-26 | upstream PR #237, #150 | NVIDIA descriptors, readback tracking | 19.7 to 20.5 fps, merged |
| 2026-11-26 | VI-BR-PORT, Vice-Tay | wave32 shadows, ray-miss skips, 4 KiB small textures, shader fixes | being ported |
| 2026-11-26 | DexNom/VIctory | "near-perfect picture" fork: the title's own ray tracing on AMD | about 1 fps, not for this PC; its quad and cube-map fix (921d8d3b) may port |
| 2026-11-26 | VI-BR-PORT 389b974 | card copies of 1 MiB to 1 GiB buffers | already covered: ours run 64 KiB to 1.5 GiB |
| 2026-11-26 | VIniVIdiVIci a53b093, Fayberr 00a21ef, Vice-Tay #41, fxceless #1359, trustno1 #7 | page-hash copy skip, idle texture freeing, snapshot pool freeing, readback reuse, readbacks on their own thread | ported, not yet measured: `VICTORY_WINDOW_SKIP_SAME` (off; `=1` or `=sample`), `VICTORY_TEXTURE_PRESSURE_IDLE`, `VICTORY_SNAPSHOT_POOL_IDLE`, `VICTORY_READBACK_POOL` (+`_MB`), `VICTORY_TEXBACK_ASYNC` (all on); the pressure parts need `VICTORY_RESIDENCY=1` (`-Lean`) |
| 2026-11-26 | VIctory-NVIDIA (pr0vId3nce) 67a50c6 | pooled own-bytes copies made outside the lock | skipped: the same as our `VICTORY_EXACT_REUSE` |
| 2026-11-26 | VI-BR-PORT a46bbdc, Fayberr 1bf62e1, vilocity bb0835a/601d5db/899ef0f, upstream #336, fxceless #837 | wave-wide refused shaders, magenta probe drop, shadow-tile barriers (orange), probe inject clear, mouths, LDS bounds, NaN guard | being ported (picture batch) |
| 2026-11-26 | own | NVIDIA-tuned shader translation: measure the translator's overhead (wave64 exchange, exec masks, packed halves, scalar emulation) and emit for 32-lane NVIDIA waves instead of mirroring RDNA (`VICTORY_NV_*`) | in progress |
| 2026-11-26 | own (idea) | the title's AMD FSR4 path asks for precompiled AMD pipelines (`DeserializeComputePipelineStateX` fails, falls back each time): see what our layer does there and give NVIDIA a cheaper path | to look at |
| 2026-11-26 | own | full-frame capture (`VICTORY_RECORD`) for shader costs | title crashes during the long stall; use skip-timing instead |
