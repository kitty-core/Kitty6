# Starts the title with the settings this fork is played with (README.md, "How to run it").
#   scripts\play.ps1 -Exe <path of your own copy's executable>
# -Safe         without the newer, faster ways (the first thing to try when something is wrong)
# -RayTracing   the title's own ray-traced indirect light: lit interiors, about a third slower
# -Build        target\diag (the default, the build all measurements are of) or target\release
# -More         other settings: -More @{VICTORY_PANEL='1'}
# -Lean         for a PC with 32 GB of RAM or less and a 12-16 GB card: the Series S profile
#               (7.7 GB of title memory, not 13.5), large textures a level smaller, small buffers on the
#               card, the residency budgets, warnings-only logging (as codec-d/cool_fox's launcher)
# -Probe        a row a second of what the PC is short of, in %LOCALAPPDATA%\victory\probes
#               (docs/probes.md; read it back with target\diag\victory.exe probe-report <file>)
# -Overlay      the Kitty6 Overlay over the game (target\<build>\kitty6-overlay.exe; F6 hides/shows it,
#               Ctrl+Alt+F6 quits; a diagnostics build feeds it; docs/probes.md)
# Run it from a normal window, not as administrator: the driver then keeps its shader cache elsewhere.
param(
    [Parameter(Mandatory)][string]$Exe,
    [switch]$Safe,
    [switch]$RayTracing,
    [switch]$Probe,
    [switch]$Overlay,
    [switch]$Lean,
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
    $settings.VICTORY_CARD_BUFFERS_MB = '1536'; $settings.VICTORY_CARD_WINDOWS_MB = '1024'
    # Upstream's renderer patches 1, 2, 5, 6 and 7, its best measured setup on the Series S
    # profile (README: 44.9 ms a frame, run merge-5).
    $settings.VICTORY_PORT_CONSTANTS = '1'; $settings.VICTORY_PORT_DESCRIPTORS = '1'
    $settings.VICTORY_PORT_CBV = '1'; $settings.VICTORY_PORT_GUARD = '1'; $settings.VICTORY_PORT_JOURNAL = '1'
    # No ray-tracing resolve pipelines without the BVH builder (as codec-d/cool_fox)
    if (-not $RayTracing) { $settings.VICTORY_RT_RESOLVE = '0' }
    $settings.VICTORY_RESIDENCY = '1'; $settings.VICTORY_LOG = 'warn'
    # No device-removal breadcrumbs: in a diagnostics build they also keep a book of
    # up to 400,000 released resources.
    $settings.Remove('VICTORY_D3D12_DRED')
}
if ($Probe) {
    $settings.VICTORY_PROBE = Join-Path $env:LOCALAPPDATA 'victory\probes'
    if ($settings.VICTORY_LOG -eq 'warn') { $settings.VICTORY_LOG = 'warn,victory_gpu::probe=info' }
}
foreach ($k in $More.Keys) { $settings[$k] = $More[$k] }

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
$console = if ($Lean) { 'series-s' } else { 'series-x' }
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
try { & $victory run $Exe --execute --console $console } finally {
    Pop-Location
    if ($overlayProcess -and -not $overlayProcess.HasExited) { Stop-Process -Id $overlayProcess.Id -ErrorAction SilentlyContinue }
}
