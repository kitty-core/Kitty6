# Kitty6 on foxhound: scripts\play-foxhound.ps1 with Kitty6's measuring and mods on.
#
#   .\scripts\kitty6.ps1                                  the title's .exe from the folder above this repository
#   .\scripts\kitty6.ps1 -Exe 'D:\MyDump\game_scarlett_master.exe'
#   .\scripts\kitty6.ps1 -Prof 240                        profile every thread for 20 s, 240 s after the start
#   .\scripts\kitty6.ps1 -SeriesX -Dlss quality           foxhound's own profile (Series X, 1440p, DLSS)
#   .\scripts\kitty6.ps1 -More @{K6_MAX_STEP_MS='0'}      add or override switches (play-foxhound -More)
#   .\scripts\kitty6.ps1 -Name step-off                   the probe's file name (default: the time)
#
# What it adds to play-foxhound:
# - the run probe (VICTORY_PROBE): one row a second in %LOCALAPPDATA%\victory\probes\<name>.csv;
#   compare two with `victory probe-report a.csv b.csv`
# - foxhound's F3 panel shown and logged once an update (VICTORY_PANEL, K6_PANEL_LOG): read the log, not the screen
# - Kitty6 renderer mods: every mods\*.k6mod of this repository (VICTORY_MODS) unless -NoMods
# - -Prof <s>: tools\kitty6-prof samples every thread's stack <s> seconds after the start, for -ProfSeconds,
#   into %LOCALAPPDATA%\victory\probes\<name>-prof.txt (build it once: cargo build --release in tools\kitty6-prof)
#
# Defaults are what this PC (32 GB RAM, ~11 GB NVIDIA card) was measured with: the diag build, Series S, no DLSS.
param(
    [string]$Exe = '',
    [ValidateSet('diag', 'release')][string]$Build = 'diag',
    [switch]$SeriesX,
    [ValidateSet('off', 'dlaa', 'quality', 'balanced', 'performance')][string]$Dlss = 'off',
    [switch]$FrameGen,
    [int]$Fps = 0,
    [string]$Name = '',
    [int]$Prof = 0,
    [int]$ProfSeconds = 20,
    [switch]$NoMods,
    [switch]$NoPanel,
    [switch]$PrepareOnly,
    [switch]$NoPrepare,
    [switch]$FullWarm,
    [hashtable]$More = @{}
)
$repo = Split-Path $PSScriptRoot -Parent
if (-not $Exe) {
    $found = @(Get-ChildItem -LiteralPath (Split-Path $repo -Parent) -Filter 'game*.exe' -File -ErrorAction SilentlyContinue)
    if ($found.Count -ne 1) { Write-Host 'Give the title''s .exe with -Exe (none, or several, in the folder above this repository).'; return }
    $Exe = $found[0].FullName
}
if (-not $Name) { $Name = 'k6-{0:yyyyMMdd-HHmmss}' -f (Get-Date) }
$probes = Join-Path $env:LOCALAPPDATA 'victory\probes'; New-Item -ItemType Directory -Force $probes | Out-Null

$k6 = @{ VICTORY_PROBE = $probes }
if (-not $NoPanel) { $k6.VICTORY_PANEL = '1'; $k6.K6_PANEL_LOG = '1' }
if (-not $NoMods) {
    $mods = @(Get-ChildItem -LiteralPath (Join-Path $repo 'mods') -Filter '*.k6mod' -File -ErrorAction SilentlyContinue | ForEach-Object FullName)
    if ($mods.Count) { $k6.VICTORY_MODS = $mods -join ';' }
}
foreach ($k in $More.Keys) { $k6[$k] = $More[$k] }

if ($Prof -gt 0 -and -not $PrepareOnly) {
    $profExe = Join-Path $repo 'target\kitty6-prof\release\kitty6-prof.exe'
    if (-not (Test-Path $profExe)) { $profExe = Join-Path $repo 'tools\kitty6-prof\target\release\kitty6-prof.exe' }
    if (Test-Path $profExe) {
        $out = Join-Path $probes "$Name-prof.txt"
        # Waits for victory.exe to start (the prepare step runs first), then for -Prof seconds of the run.
        Start-Job -ArgumentList $profExe, $out, $Prof, $ProfSeconds -ScriptBlock {
            param($profExe, $out, $after, $secs)
            while (-not (Get-Process victory -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 })) { Start-Sleep 2 }
            Start-Sleep $after
            & $profExe --seconds $secs --hz 50 --out $out | Out-Null
        } | Out-Null
        Write-Host "Profile: $out ($ProfSeconds s, $Prof s after the start)"
    } else { Write-Host 'No kitty6-prof build (cargo build --release in tools\kitty6-prof): running without -Prof.' }
}

$fox = @{ Exe = $Exe; Build = $Build; Dlss = $Dlss; More = $k6 }
if (-not $SeriesX) { $fox.SeriesS = $true }
if ($FrameGen) { $fox.FrameGen = $true }
if ($Fps -gt 0) { $fox.Fps = $Fps }
if ($PrepareOnly) { $fox.PrepareOnly = $true }
if ($NoPrepare) { $fox.NoPrepare = $true }
if ($FullWarm) { $fox.FullWarm = $true }
$start = Get-Date
& (Join-Path $PSScriptRoot 'play-foxhound.ps1') @fox
# The probe names its file probe-<unix seconds>.csv: the run's is given -Name.
$csv = Get-ChildItem -LiteralPath $probes -Filter 'probe-*.csv' -File -ErrorAction SilentlyContinue |
    Where-Object LastWriteTime -gt $start | Sort-Object LastWriteTime | Select-Object -Last 1
if ($csv) { $to = Join-Path $probes "$Name.csv"; Move-Item -Force $csv.FullName $to; Write-Host "Probe: $to" }
