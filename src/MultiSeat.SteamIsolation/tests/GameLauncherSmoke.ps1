param(
    [string]$Artifacts = (Join-Path $PSScriptRoot '..\artifacts'),
    [string]$TestRoot = (Join-Path $PSScriptRoot '..\build-game-smoke')
)
$ErrorActionPreference = 'Stop'
$taskSource = [IO.Path]::GetFullPath($Artifacts)
$taskRun = Join-Path ([IO.Path]::GetFullPath($TestRoot)) ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $taskRun -Force | Out-Null
$taskFixture = Join-Path $taskSource 'SteamIsolationFixture.exe'
foreach ($taskName in @('MultiSeat.SteamLauncher.exe', 'MultiSeat.SteamHook.dll', 'MultiSeat.GameFiles.dll')) {
    Copy-Item -LiteralPath (Join-Path $taskSource $taskName) -Destination $taskRun
}
Copy-Item -LiteralPath $taskFixture -Destination (Join-Path $taskRun 'steam.exe')
Copy-Item -LiteralPath $taskFixture -Destination (Join-Path $taskRun 'game.exe')
$taskLauncher = $null
try {
    $taskSteamReport = Join-Path $taskRun 'steam-report.txt'
    $taskSteamArgs = '--steam "' + (Join-Path $taskRun 'steam.exe') + '" -- --report "' + $taskSteamReport + '" --hold-ms 30000'
    $taskLauncher = Start-Process -FilePath (Join-Path $taskRun 'MultiSeat.SteamLauncher.exe') -ArgumentList $taskSteamArgs -WorkingDirectory $taskRun -WindowStyle Hidden -PassThru
    $taskDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (Test-Path -LiteralPath $taskSteamReport) -and [DateTime]::UtcNow -lt $taskDeadline) { Start-Sleep -Milliseconds 100 }
    if (-not (Test-Path -LiteralPath $taskSteamReport)) { throw 'Isolated Steam fixture did not start.' }
    $taskOriginal = $env:steam_master_ipc_name_override
    & (Join-Path $PSScriptRoot '..\Launch-IsolatedGame.ps1') -Executable (Join-Path $taskRun 'game.exe') -Arguments '--report game-report.txt'
    if ($env:steam_master_ipc_name_override -cne $taskOriginal) { throw 'Game launcher changed the caller environment.' }
    $taskGameReport = Join-Path $taskRun 'game-report.txt'
    $taskDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (Test-Path -LiteralPath $taskGameReport) -and [DateTime]::UtcNow -lt $taskDeadline) { Start-Sleep -Milliseconds 100 }
    if (-not (Test-Path -LiteralPath $taskGameReport)) { throw 'Game did not write its report in its installation folder.' }
    $taskSteamIPC = @(Get-Content -LiteralPath $taskSteamReport | Where-Object { $_ -like 'steam_master_ipc_name_override=*' })
    $taskGameIPC = @(Get-Content -LiteralPath $taskGameReport | Where-Object { $_ -like 'steam_master_ipc_name_override=*' })
    if ($taskSteamIPC.Count -ne 1 -or $taskGameIPC.Count -ne 1 -or $taskSteamIPC[0] -cne $taskGameIPC[0]) { throw 'Game and isolated Steam used different IPC names.' }
    if (-not (Select-String -LiteralPath $taskGameReport -Pattern '^SteamPID=123456$' -Quiet)) { throw 'Direct game received an unexpected registry hook.' }
    Write-Host 'Game launcher environment and working-directory checks passed.'

    # Faux Longshot : son parent ne lit ni n'écrit de sauvegarde ; seul son enfant suspendu le fait.
    $taskHostGame = Join-Path $taskRun 'longshot'
    $taskPlayer = Join-Path $taskRun 'player'
    $taskHostSave = Join-Path $taskHostGame 'user\save\hote\slot_01'
    New-Item -ItemType Directory -Path $taskHostSave, $taskPlayer -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $taskSource 'fakelongshot.exe') -Destination $taskHostGame
    [IO.File]::WriteAllText((Join-Path $taskHostSave 'progress.sav'), 'sauvegarde hote')
    [IO.File]::WriteAllText((Join-Path $taskHostGame 'cache_origine.bin'), 'cache hote')
    $taskManifestFile = Join-Path $taskRun 'longshot-manifest.json'
    $taskGameLog = Join-Path $taskRun 'longshot-gamefiles.log'
    $taskLongshotReport = Join-Path $taskRun 'longshot-report.txt'
    $taskManifest = @{
        gameId = 'longshot'; account = 'player'
        accountSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        profileDir = $taskPlayer; logFile = $taskGameLog; copyFromHostOnce = $false
        redirects = @(
            @{ from = (Join-Path $taskHostGame 'user\save'); to = (Join-Path $taskPlayer 'save'); mode = 'redirect' },
            @{ from = $taskHostGame; to = (Join-Path $taskPlayer 'overlay'); mode = 'overlay' }
        )
    }
    [IO.File]::WriteAllText($taskManifestFile, ($taskManifest | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
    # Le lanceur est une application fenêtrée : PowerShell n'attend pas son code de sortie avec `&`.
    $taskLongshotArgs = '--game "' + $taskManifestFile + '" -- "' + (Join-Path $taskHostGame 'fakelongshot.exe') + '" "' + $taskLongshotReport + '" --relaunch-suspended'
    $taskLongshot = Start-Process -FilePath (Join-Path $taskRun 'MultiSeat.SteamLauncher.exe') -ArgumentList $taskLongshotArgs -WorkingDirectory $taskRun -WindowStyle Hidden -PassThru -Wait
    if ($taskLongshot.ExitCode -ne 0) { throw "Longshot suspendu : code de sortie $($taskLongshot.ExitCode)" }
    if (-not (Test-Path -LiteralPath $taskLongshotReport)) { throw "Longshot enfant n'a pas écrit son rapport." }
    $taskReport = [IO.File]::ReadAllText($taskLongshotReport)
    if ($taskReport -match 'slot_01' -or $taskReport -notmatch 'user\\save absent' -or
        $taskReport -notmatch 'cache hote') { throw "Longshot enfant a vu les sauvegardes de l'hôte ou perdu le cache." }
    if (-not (Test-Path -LiteralPath (Join-Path $taskPlayer 'save\joueur\slot_00\progress.sav'))) { throw "Longshot enfant n'a pas écrit chez le joueur." }
    if ((Test-Path -LiteralPath (Join-Path $taskHostGame 'user\save\joueur')) -or
        [IO.File]::ReadAllText((Join-Path $taskHostSave 'progress.sav')) -cne 'sauvegarde hote') { throw 'Longshot a modifié les sauvegardes hôte.' }
    $taskLog = [IO.File]::ReadAllText($taskGameLog)
    if ([regex]::Matches($taskLog, 'Redirection de fichiers active').Count -lt 2 -or
        $taskLog.Contains('injection différée non prise en charge')) { throw 'Injection enfant suspendu absente ou différée.' }
    Write-Host 'Suspended Longshot relaunch isolation checks passed.'
} finally {
    if ($taskLauncher -and -not $taskLauncher.HasExited) {
        Stop-Process -Id $taskLauncher.Id
        [void]$taskLauncher.WaitForExit(5000)
    }
    if ($taskLauncher) { $taskLauncher.Dispose() }
}
