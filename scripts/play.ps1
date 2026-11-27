# Starts the title with the settings this fork is played with (README.md, "How to run it").
#   scripts\play.ps1 -Exe <path of your own copy's executable>
# -Safe         without the newer, faster ways (the first thing to try when something is wrong)
# -RayTracing   the title's own ray-traced indirect light: lit interiors, about a third slower
# -Build        target\diag (the default, the build all measurements are of) or target\release
# -More         other settings: -More @{VICTORY_PANEL='1'}
# -Lean         for a PC with 32 GB of RAM or less and a 12-16 GB card: the Series S profile
#               (7.7 GB of title memory, not 13.5), large textures a level smaller, small buffers on the
#               card, the residency budgets, warnings-only logging (as codec-d/cool_fox's launcher)
# -Rtx4070      opt-in RTX 4070 (12 GB) / 8-core preset, meant beside -Lean (TaynanGT/Vice-Tay PR #69):
#               only what -Lean and the defaults don't already set: 4 landing threads and 3 pipeline
#               workers fixed, and the texture queue keeping the card buffer/window budgets in full
#               (VICTORY_CARD_RESERVE_BUDGETS=1, not under -Safe)
# -Probe        a row a second of what the PC is short of, in %LOCALAPPDATA%\victory\probes
#               (docs/probes.md; read it back with target\diag\victory.exe probe-report <file>)
# -Overlay      the Kitty6 Overlay over the game (target\<build>\kitty6-overlay.exe; F6 hides/shows it,
#               Ctrl+Alt+F6 quits; a diagnostics build feeds it; docs/probes.md)
# -Fps          uncapped frame rate: the title's 30 fps cap lifted to this many a second (60, 120;
#               0 = no cap at all). Off by default: 30 is the game as made (VICTORY_FRAME_INTERVAL_MS)
# -Quality      Low, Medium or High: texture detail (the top level of large textures left out at
#               Low and Medium, a second one too at Low; Medium is what -Lean sets)
# -Drive        scripted gameplay from -DriveAt seconds (default 200): walks, turns, runs and jumps for
#               about a minute without your hands (diagnostics build), so measurements see moving play
# -Console      X or S: the console profile, whatever -Lean picks (-Lean alone is S). X is the
#               sharper Series X picture; with -Output 1920x1080 it renders 1080p itself
# -Output       the title's own output size, e.g. 1920x1080 (Series X profile; VICTORY_OUTPUT)
# -Dev          dev-menu items at set times, e.g. '200:footage,240:timeline,270:autopilot' (seconds after
#               the first frame; VICTORY_DEV). In game F4 opens the dev menu: PgUp/PgDn pick, Insert runs.
#               Items: panel, footage (frames saved as PNG to %LOCALAPPDATA%\victory\footage), rate, shot,
#               timeline, snapshot (numbers to the log), autopilot (walk/run/circle), mark
# -Panel        the run's numbers drawn on the picture (F3 hides and shows them), so screenshots carry them
# -F8Timeline   F8 in the game records a 3-frame timeline (diagnostics build) to logs\f8.f8-<n>.json/.txt:
#               where the frame's time went, at the moment you press it
# Run it from a normal window, not as administrator: the driver then keeps its shader cache elsewhere.
param(
    [Parameter(Mandatory)][string]$Exe,
    [switch]$Safe,
    [switch]$RayTracing,
    [switch]$Probe,
    [switch]$Overlay,
    [switch]$Lean,
    [switch]$Rtx4070,
    [double]$Fps = -1,
    [ValidateSet('', 'Low', 'Medium', 'High')][string]$Quality = '',
    [switch]$Drive,
    [double]$DriveAt = 200,
    [switch]$Panel,
    [string]$Dev = '',
    [ValidateSet('', 'X', 'S')][string]$Console = '',
    [string]$Output = '',
    [switch]$F8Timeline,
    [switch]$NoMods,
    [ValidateSet('diag', 'release')][string]$Build = 'diag',
    [hashtable]$More = @{}
)
$settings = @{
    VICTORY_TEXTURE_NO_WAIT = '1'; VICTORY_RELAY = '2'; VICTORY_WIDE_BACK = '1'; VICTORY_CARD_REHASH_RUNS = '1'
    VICTORY_JOIN_COPIES = '1'; VICTORY_INDIRECT_WINDOW = '1'; VICTORY_RELAY_COPY = '1'; VICTORY_CARD_STEPS = '2'
    VICTORY_INDIRECT_BATCH = '2'; VICTORY_MAP_THREADS = '1'; VICTORY_TITLE_PRIORITIES = '0'; VICTORY_CARD_AWAKE = '1'
    VICTORY_SHOW_AFTER = '1'; VICTORY_AMBIENT = '0.8'; VICTORY_D3D12_DRED = 'light'
    # Streamed voices (dialogue) go on when the title refills them, as codec-d/cool_fox
    # always does: without it speech stalls, and lip sync with it.
    VICTORY_XMA_REFILL = '1'
    # Halves (16-bit values) go by the float path: the 16-bit views and loads made sun
    # shadows and faces blocky on NVIDIA (stair-stepped shadow edges, square cut-outs;
    # gone with both off, 2026-11-26). -More @{VICTORY_HALF_VIEWS='1'; K6_HALF_LOADS='1'} to try them.
    VICTORY_HALF_VIEWS = '0'; K6_HALF_LOADS = '0'
    # mimalloc hands freed pages back to Windows after 10 ms and takes them again as
    # fresh zeroed pages (~45,000 faults in 12 s in a fault trace): keep them 1 s.
    MIMALLOC_PURGE_DELAY = '1000'
}
if ($Safe) {
    $settings.VICTORY_UNDER_WIDE = '0'; $settings.VICTORY_LAND_ECHO = '0'
} else {
    $settings.VICTORY_THIN = '1'; $settings.VICTORY_JOIN_PARTS = '8'; $settings.VICTORY_UPLOAD_AHEAD = '1'
}
if ($RayTracing) { $settings.VICTORY_BVH_BUILDER = '1' }
if ($Lean) {
    # Held levels with the top level of large textures left out (not levels on demand,
    # which turns held levels off), the card's texture pass stopped at 80 %.
    $settings.VICTORY_TEXTURE_MIP_BIAS = '1'; $settings.VICTORY_TEXTURE_ROOM_PERCENT = '80'
    $settings.VICTORY_CARD_BUFFERS_MIN_KB = '64'
    # Buffer and window copies on the card's unused memory instead of in RAM: in the
    # world about 4 -> 15-20 fps on a 32 GB PC with an 11 GB card (run mix4).
    # Windows on the card keep no full copy in RAM (K6_WINDOW_NO_CPU_SIDE): each MB of
    # them is a MB less of private memory, so twice the windows go there.
    # -More @{K6_WINDOW_NO_CPU_SIDE='0'; VICTORY_CARD_WINDOWS_MB='1024'} for the old setup.
    $settings.VICTORY_CARD_BUFFERS_MB = '1536'; $settings.VICTORY_CARD_WINDOWS_MB = '2048'
    $settings.K6_WINDOW_NO_CPU_SIDE = '1'
    # Upstream's renderer patches 1, 2, 5, 6 and 7, its best measured setup on the Series S
    # profile (README: 44.9 ms a frame, run merge-5).
    $settings.VICTORY_PORT_CONSTANTS = '1'; $settings.VICTORY_PORT_DESCRIPTORS = '1'
    $settings.VICTORY_PORT_CBV = '1'; $settings.VICTORY_PORT_GUARD = '1'; $settings.VICTORY_PORT_JOURNAL = '1'
    # No ray-tracing resolve pipelines without the BVH builder (as codec-d/cool_fox)
    if (-not $RayTracing) { $settings.VICTORY_RT_RESOLVE = '0' }
    # Without rays the title's indirect light is black indoors and leaves blocks of stale probe light:
    # the stand-in estimates it from the frame (ambient.rs). Closer to the console's picture, at no
    # measured cost (runs J/K-ambient, 2026-11-26).
    if (-not $RayTracing) { $settings.VICTORY_AMBIENT = '1' }
    # Off the serial path (Peppacodes/playablegames-peppa PRs 2 and 6, measured on an RTX 5080):
    # host objects that waited for a flight dropped on a thread of their own, not on the landing
    # thread (-23 ms mean there), and landed lists reset on one (-2 to -4 ms there).
    $settings.VICTORY_BURY_ASIDE = '1'; $settings.VICTORY_LAND_ASIDE = '1'
    $settings.VICTORY_RESIDENCY = '1'; $settings.VICTORY_LOG = 'warn'
    # No device-removal breadcrumbs: in a diagnostics build they also keep a book of
    # up to 400,000 released resources.
    $settings.Remove('VICTORY_D3D12_DRED')
}
if ($Rtx4070) {
    # From TaynanGT/Vice-Tay PR #69 (266ba60a, zzzz-26) and f3dfb5b9 (TaynanGT), for an RTX 4070
    # with an 8-core Ryzen 7 7800X3D. Their 8 list threads, 6 under-write threads and 1 mapping
    # thread are already our defaults/base settings, and their 64 KiB / 1536 MB / 1024 MB card
    # budgets are -Lean's, so only these differ: landing threads fixed at 4 (ours: a quarter of the
    # logical processors, 1-4) and 3 pipeline workers (ours: two thirds of them, 2-16), leaving
    # the cores to the title.
    $settings.VICTORY_LAND_THREADS = '4'; $settings.VICTORY_PIPELINE_WORKERS = '3'
    if (-not $Safe) {
        # The explicit buffer budget kept in full for card copies (not 1 GiB at most), the window
        # budget kept too, and windows to the card up to 97 % of its budget instead of 80 %.
        $settings.VICTORY_CARD_RESERVE_BUDGETS = '1'
    }
    if (-not $Lean) { Write-Host '-Rtx4070 is meant beside -Lean (its card budgets are what the reservation keeps).' }
}
if ($Probe) {
    $settings.VICTORY_PROBE = Join-Path $env:LOCALAPPDATA 'victory\probes'
    if ($settings.VICTORY_LOG -eq 'warn') { $settings.VICTORY_LOG = 'warn,victory_gpu::probe=info' }
}
if ($Quality) { $settings.VICTORY_TEXTURE_MIP_BIAS = @{ Low = '2'; Medium = '1'; High = '0' }[$Quality] }
if ($Fps -eq 0) { $settings.VICTORY_FRAME_INTERVAL_MS = '0' }
elseif ($Fps -gt 0) { $settings.VICTORY_FRAME_INTERVAL_MS = (1000.0 / $Fps).ToString('0.###', [Globalization.CultureInfo]::InvariantCulture) }
if ($Panel) { $settings.VICTORY_PANEL = '1' }
if ($Dev) { $settings.VICTORY_DEV = $Dev }
if ($Output) { $settings.VICTORY_OUTPUT = $Output }
# Kitty6 renderer mods: the repository's mods\*.k6mod rule files (crates/victory-gpu/src/kitty6.rs).
if (-not $NoMods) {
    $modFiles = @(Get-ChildItem -Path (Join-Path $PSScriptRoot '..\mods') -Filter '*.k6mod' -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
    if ($modFiles.Count) { $settings.VICTORY_MODS = $modFiles -join ';' }
}
if ($F8Timeline) { $settings.VICTORY_TIMELINE = 'key,3,' + (Join-Path $env:LOCALAPPDATA 'victory\logs\f8.json') }
foreach ($k in $More.Keys) { $settings[$k] = $More[$k] }
if ($Drive) {
    # Forward, a turn each way, a run (Shift) and a jump (Space), over and over: seconds after input starts.
    $t = $DriveAt; $keys = @()
    for ($i = 0; $i -lt 4; $i++) {
        $keys += "$($t)-$($t + 6):W", "$($t + 6)-$($t + 7.5):A", "$($t + 7.5)-$($t + 13):W", "$($t + 7.5)-$($t + 13):Shift",
                 "$($t + 13)-$($t + 14.5):D", "$($t + 14.5):Space", "$($t + 14.5)-$($t + 15.5):W"
        $t += 16
    }
    $settings.VICTORY_KEYS = (@($settings.VICTORY_KEYS) + $keys | Where-Object { $_ }) -join ','
}


$victory = Join-Path $PSScriptRoot "..\target\$Build\victory.exe"
if (-not (Test-Path $victory)) { Write-Host "No $victory. Build it first (README.md)."; exit 1 }
if (-not (Test-Path $Exe)) { Write-Host "No $Exe."; exit 1 }
# (Other settings left over in this window go; where your files are stays.)
$paths = 'VICTORY_CONFIG', 'VICTORY_GAME_DIR', 'VICTORY_CACHE_DIR', 'VICTORY_DATA_DIR'
$inherited = @(Get-ChildItem Env:VICTORY_* | Where-Object { $_.Name -notin $paths })
foreach ($entry in $inherited) { Remove-Item "Env:$($entry.Name)" }
foreach ($k in $settings.Keys) { Set-Item "Env:$k" $settings[$k] }
$discarded = $inherited | Where-Object {
    $_.Name -notin $More.Keys -and -not $_.Value.Equals([Environment]::GetEnvironmentVariable($_.Name))
} | Sort-Object Name
if ($discarded) {
    Write-Host "Discarded inherited values for $($discarded.Name -join ', '). Use -More to pass them explicitly."
}
$Exe = (Resolve-Path $Exe).Path
Push-Location (Join-Path $PSScriptRoot '..')   # victory.toml is looked for here
$consoleProfile = if ($Console) { @{ X = 'series-x'; S = 'series-s' }[$Console] } elseif ($Lean) { 'series-s' } else { 'series-x' }
$overlayProcess = $null
if ($Overlay) {
    $overlayExe = Join-Path $PSScriptRoot "..\target\$Build\kitty6-overlay.exe"
    if (Test-Path $overlayExe) {
        # It quits by itself once the game has gone; stopped below as well.
        $overlayProcess = Start-Process -FilePath $overlayExe -ArgumentList '--exit-with-game' -WindowStyle Hidden -PassThru
    } else {
        Write-Host "No $overlayExe. Build it: cargo build --profile $Build -p kitty6-overlay"
    }
}
try { & $victory run $Exe --execute --console $consoleProfile } finally {
    Pop-Location
    if ($overlayProcess -and -not $overlayProcess.HasExited) { Stop-Process -Id $overlayProcess.Id -ErrorAction SilentlyContinue }
}
