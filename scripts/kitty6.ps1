# Kitty6 launcher: runs scripts\play.ps1 -Lean with a preset, then any single switches.
#
#   scripts\kitty6.ps1 -Exe <your copy's executable>                 # Current: today's -Lean
#   scripts\kitty6.ps1 -Exe <exe> -Preset Best                       # the best measured run (pc1, 18.8 fps)
#   scripts\kitty6.ps1 -Exe <exe> -Preset Best -On LeanWindows       # Best plus one change
#   scripts\kitty6.ps1 -Exe <exe> -Off HardWorkingSet,LeanBuffers    # Current minus some changes
#   scripts\kitty6.ps1 -List                                         # the switches by name
#
# The log goes to %LOCALAPPDATA%\victory\logs\kitty6-<time>.log; -Console prints it instead.
# -Exe can be left out once K6_EXE is set to the executable's path. -Probe and
# -Panel are passed through; anything else goes in -More @{NAME='value'}.
param(
    [string]$Exe = $env:K6_EXE,
    [ValidateSet('Current', 'Best')][string]$Preset = 'Current',
    [string[]]$On = @(),
    [string[]]$Off = @(),
    [hashtable]$More = @{},
    [switch]$Probe,
    [switch]$Panel,
    [switch]$Console,
    [switch]$List
)

# Each change since the best run: the settings that turn it on, and those that turn it off.
$changes = [ordered]@{
    HardWorkingSet = @{ On = @{ K6_HARD_WORKING_SET = '1' };   Off = @{ K6_HARD_WORKING_SET = '0' };   What = 'a hard minimum working set, so Windows does not trim the game' }
    PurgeDelay     = @{ On = @{ MIMALLOC_PURGE_DELAY = '1000' }; Off = @{ MIMALLOC_PURGE_DELAY = '10' }; What = 'freed memory kept 1 s before going back to Windows' }
    LeanWindows    = @{ On = @{ K6_WINDOW_NO_CPU_SIDE = '1'; VICTORY_CARD_WINDOWS_MB = '2048' }; Off = @{ K6_WINDOW_NO_CPU_SIDE = '0'; VICTORY_CARD_WINDOWS_MB = '1024' }; What = 'card copies of title memory without a RAM copy' }
    LeanBuffers    = @{ On = @{ K6_BUFFER_NO_CPU_SIDE = '1' }; Off = @{ K6_BUFFER_NO_CPU_SIDE = '0' }; What = 'card buffer copies filled once without a RAM copy (after 30 s)' }
    SpareSend      = @{ On = @{ K6_SEND_SPARE_S = '10' };      Off = @{ K6_SEND_SPARE_S = '0' };      What = 'idle spare upload memory let go after 10 s' }
    Aside          = @{ On = @{ VICTORY_BURY_ASIDE = '1'; VICTORY_LAND_ASIDE = '1' }; Off = @{ VICTORY_BURY_ASIDE = '0'; VICTORY_LAND_ASIDE = '0' }; What = 'landed objects dropped and lists reset on threads of their own' }
    StagingSettle  = @{ On = @{ VICTORY_STAGING_SETTLE_S = '30' }; Off = @{}; What = 'settled textures give up their staging (off by default)' }
    ListsIdle      = @{ On = @{ VICTORY_LIST_POOL_IDLE_S = '30' }; Off = @{}; What = 'idle host lists let go (off by default)' }
    ExactWave64    = @{ On = @{ K6_EXACT_WAVE64 = '1' };       Off = @{};                             What = 'exact wave64 for 34 shadow/volumetric programs (off by default)' }
    PairedWave64   = @{ On = @{ VICTORY_WAVE_PAIRED_ONLY = '1aa247ed3653b616,5dc3b25986622949,6871d6e4b3ac9a10,6f225b641c5b0708,926c46bab3b86bd6' }; Off = @{}; What = 'the five water volumetrics programs in the paired form (water patches; =all lost the card here; off by default)' }
}
# On in today's -Lean; Best turns these off.
$inLean = 'HardWorkingSet', 'PurgeDelay', 'LeanWindows', 'LeanBuffers', 'SpareSend', 'Aside'

if ($List) {
    foreach ($k in $changes.Keys) { '{0,-15} {1}{2}' -f $k, $changes[$k].What, $(if ($inLean -contains $k) { '  [in Current]' } else { '' }) }
    return
}
if (-not $Exe) { throw 'Pass -Exe <your copy''s executable>, or set K6_EXE.' }

$settings = @{}
if ($Preset -eq 'Best') { foreach ($k in $inLean) { $changes[$k].Off.GetEnumerator() | ForEach-Object { $settings[$_.Key] = $_.Value } } }
foreach ($k in $Off) { if (-not $changes.Contains($k)) { throw "unknown switch $k (see -List)" }; $changes[$k].Off.GetEnumerator() | ForEach-Object { $settings[$_.Key] = $_.Value } }
foreach ($k in $On)  { if (-not $changes.Contains($k)) { throw "unknown switch $k (see -List)" }; $changes[$k].On.GetEnumerator()  | ForEach-Object { $settings[$_.Key] = $_.Value } }
# The log goes to a file unless -Console: printing to a console is slow.
if (-not $Console) {
    $logs = Join-Path $env:LOCALAPPDATA 'victory\logs'; New-Item -ItemType Directory -Force $logs | Out-Null
    $settings.K6_LOG_FILE = Join-Path $logs ('kitty6-{0:yyyyMMdd-HHmmss}.log' -f (Get-Date))
    "log: $($settings.K6_LOG_FILE) (-Console to print it here)"
}
$More.GetEnumerator() | ForEach-Object { $settings[$_.Key] = $_.Value }

"Kitty6: preset $Preset" + $(if ($On) { ", on: $($On -join ',')" }) + $(if ($Off) { ", off: $($Off -join ',')" })
$play = @{ Exe = $Exe; Lean = $true; More = $settings }
if ($Probe) { $play.Probe = $true }
if ($Panel) { $play.Panel = $true }
& (Join-Path $PSScriptRoot 'play.ps1') @play
