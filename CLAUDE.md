# Kitty6: how to work here

This tree is Kitty6, a fork of VIctory tuned to run one Xbox Series title well
on a PC with 32 GB of RAM and an ~11 GB NVIDIA card. The aim is a stable,
good-looking picture at the highest frame rate this PC can reach.

## Be determined

- Use any means that makes the game run better: change any crate, port work
  from other forks (credit the commit and author), or write our own shaders and
  tools in Rust. Don't stop at the first gain. Measure, change, measure again,
  and keep going until it is better and faster.
- Act, then report. Code changes, builds and test runs in this tree need no
  approval.
- Keep the picture intact. A speed-up that breaks rendering (static faces,
  missing effects) is a bug to chase down, not a trade to accept quietly.
  Put a risky change behind a `VICTORY_...` switch, described in a comment where
  it is read.

## How to measure

- Build: `cargo build --locked --profile diag -p victory --features diagnostics`.
  Tests: `cargo test --locked --profile diag -p victory-gpu --features diagnostics --lib`.
- Run: `scripts\play.ps1 -Exe <exe> -Lean -Probe`, with `-More @{...}` for
  switches. Compare runs with `victory probe-report <a.csv> <b.csv>`
  ([docs/probes.md](docs/probes.md)). A run spends only about 30-60 s in the world,
  so small differences are noise: look for large, consistent ones.
- `victory warm --used` prebuilds pipelines after a build or driver change.
  It locks `target\diag\victory.exe`, and so does a running game. Rename the
  running exe aside before rebuilding.
- Run games and builds in the background, never as long blocking waits, and
  hand research or ports to subagents (ports in their own worktree).
- Don't end a run the user is playing: stopping the launcher ends the game.

## Where the time goes on this PC

Memory, not the CPU or GPU: RAM paging and card memory. What helped most so far
(2026-11-26): buffer copies on the card (`VICTORY_CARD_BUFFERS_MB=1536`,
`VICTORY_CARD_WINDOWS_MB=1024`), about 4 to 15-20 fps in the world. Tried and
worse: 3 GB caps for both (2.5 GB more private memory, more paging), finer window
copies (`VICTORY_WINDOW_KB`, `VICTORY_WINDOW_TAILS`), small buffers off the card
(`VICTORY_CARD_BUFFERS_MIN_KB=16384`). `VICTORY_INPLACE` can't work here (the
driver lacks `OpenExistingHeapFromAddress1`). `VICTORY_MOVIE_PLANES` (on) is what
keeps the menu videos smooth.

Then (same day) a lock, found with `tools/kitty6-prof`: every miss in
`shadow::exact` (copies of buffer views' own bytes, new ring positions every
frame) made a buffer and waited for the card to map it (`host::mapped`) while
holding the global `EXACT` lock. The 8 recording threads spent ~75% queued on it
(3-5 fps). Buffers of copies let go of are now used again by size, made outside
the lock (`VICTORY_EXACT_REUSE=0`: as before): 18-19 fps in the opening scene.

Where the 50 ms frame goes after that (runs O-why, S-tl, T-submit):
- The submitting thread is the serial path: busy ~55%, waiting for the card
  ~35-40% in settles. Recording (8 list threads, `execute_indirect` and
  `State::bind`) keeps only ~1-2 threads busy; the wide/land pools idle ~90%.
- Settles: a fill of upload memory reading bytes a shader writes every frame
  (~21 ms). With `VICTORY_FILL_STALE_BYTES` (opt-in) the wait moves on to
  "what a copy at its place writes" (~12 ms): +1 fps only. All of them have
  to go before preparing and the card overlap.
- Serial work in `Host::submit`: `Batch::flush_one`/`update` memcpy
  (`victory_safe_copy_at`), `refresh_spans`, `send::take`,
  `stale_under_flights`, write-watch scans.
- `VICTORY_TIMELINE=<s>,<frames>,<file.json>` writes a Chrome trace and a
  text summary of the critical chain per frame (heavy while it records).

## Profiling tools

- `tools/kitty6-prof` (own workspace: `cargo build --release --offline` there):
  samples every thread's stack of the running victory.exe, e.g.
  `kitty6-prof --seconds 20 --hz 50 --out prof.txt`. Per thread: running share,
  what it runs or waits in, call chains. Names come from the PDB beside the exe;
  if the exe was renamed aside and rebuilt, resolve afterwards with
  `kitty6-prof --resolve target\diag\victory.exe 0xOFFSET...` (the build that ran).
- `apps/kitty6-overlay` (`scripts\play.ps1 -Overlay`, F6): the live HUD.

Known open issues: mouths don't animate (not caused by any switch added on
2026-11-26; parked until the frame rate is up), and six compute shaders are
refused on NVIDIA because they are translated for 64-lane waves.

Keep in step with upstream (`git fetch origin`, avatarmanz/VIctory-7900XTX): its
renderer patches (`VICTORY_PORT_*`) are on in `-Lean`.
