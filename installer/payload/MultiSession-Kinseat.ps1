#Requires -Version 5.1
# Plusieurs sessions Windows en même temps (RDP Wrapper, expérimental).
# Active ou retire RDP Wrapper sur demande explicite de l'utilisateur (case du setup ou
# Paramètres de l'application Hôte). Jamais appelé implicitement.
#   -Action Enable  : télécharge RDP Wrapper 1.6.2 (stascorp) et l'ini communautaire (sebaxakerhtc) ;
#                     utilise les offsets livrés seulement pour une empreinte termsrv.dll validée.
#   -Action Disable : retire RDP Wrapper s'il a été installé par Kinseat, restaure termsrv.dll.
# Sortie : lignes JSON (step, status, message, pct) dans -Progress ; code 0 = succès.
param(
    [Parameter(Mandatory=$true)][ValidateSet('Enable','Disable')][string]$Action,
    [string]$Progress = (Join-Path $env:ProgramData 'Kinseat\multisession.jsonl'),
    [string]$Log = (Join-Path $env:ProgramData 'Kinseat\multisession.log'),
    [switch]$UpdateState
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-StrictMode -Version Latest

$rdp = Join-Path $env:ProgramFiles 'RDP Wrapper'
$data = Join-Path $env:ProgramData 'Kinseat'
$work = Join-Path $data 'multisession-work'
$marker = Join-Path $rdp 'kinseat-owned.json'
$stateFile = Join-Path $data 'install-state.json'
$params = 'HKLM:\SYSTEM\CurrentControlSet\Services\TermService\Parameters'
$sources = [ordered]@{
    wrapper = @{ url = 'https://github.com/stascorp/rdpwrap/releases/download/v1.6.2/RDPWrap-v1.6.2.zip'; sha = '35A9481DDBED5177431A9EA4BD09468FE987797D7B1231D64942D17EB54EC269'; file = 'rdpwrap.zip' }
    ini     = @{ url = 'https://raw.githubusercontent.com/sebaxakerhtc/rdpwrap.ini/9a1356fc7ff2bf7d6d02c32a1d337dcd069e843c/rdpwrap.ini'; sha = '28F6580EDF531CD9665B45CE59F41526D4A94BFFFAB5DF833D47E3AA3261A81A'; file = 'rdpwrap.ini' }
}

New-Item -ItemType Directory -Path $data -Force | Out-Null
foreach ($f in @($Progress, $Log)) { $dir = Split-Path $f -Parent; if ($dir) { New-Item -ItemType Directory -Path $dir -Force | Out-Null } }
[IO.File]::WriteAllText($Progress, '')

function Emit($step, $status, $message, $pct) {
    $row = [ordered]@{ step = $step; status = $status; message = $message; pct = $pct; utc = [DateTime]::UtcNow.ToString('o') }
    $line = (ConvertTo-Json -Compress -InputObject $row) + "`r`n"
    $utf8 = New-Object Text.UTF8Encoding($false)
    [IO.File]::AppendAllText($Progress, $line, $utf8)
    [IO.File]::AppendAllText($Log, "$($row.utc) | $Action | $step | $status | $message`r`n", $utf8)
}
function Service-Dll { (Get-ItemProperty -LiteralPath $params -Name ServiceDll -ErrorAction SilentlyContinue).ServiceDll }
function Is-Stock($dll) { -not $dll -or ([Environment]::ExpandEnvironmentVariables($dll) -match '\\System32\\termsrv\.dll$') }
function Termsrv-Version { (Get-Item (Join-Path $env:SystemRoot 'System32\termsrv.dll')).VersionInfo.FileVersionRaw.ToString() }
function Ini-Section($text, $name) {
    $escaped = [regex]::Escape($name)
    $sections = [regex]::Matches($text, "(?m)^\[$escaped\][ \t]*\r?\n(?<body>(?:(?!^\[).*(?:\r?\n|$))*)")
    if ($sections.Count -ne 1) { return $null }
    return $sections[0]
}
function Ini-Covers($text, $version) {
    $main = Ini-Section $text $version
    $init = Ini-Section $text "$version-SLInit"
    if (-not $main -or -not $init) { return $false }
    foreach ($key in @('SingleUserPatch','SingleUserOffset','SingleUserCode',
            'DefPolicyPatch','DefPolicyOffset','DefPolicyCode',
            'LocalOnlyPatch','LocalOnlyOffset','LocalOnlyCode',
            'SLInitHook','SLInitOffset','SLInitFunc')) {
        if ($main.Groups['body'].Value -notmatch "(?m)^$key\.x64\s*=\s*\S+") { return $false }
    }
    foreach ($key in @('bServerSku','bRemoteConnAllowed','bFUSEnabled','bAppServerAllowed',
            'bMultimonAllowed','lMaxUserSessions','ulMaxDebugSessions','bInitialized')) {
        if ($init.Groups['body'].Value -notmatch "(?m)^$key\.x64\s*=\s*\S+") { return $false }
    }
    return $true
}
function Select-ValidatedOffsets($text, $version, $hash) {
    if (-not (Ini-Covers $text $version)) { return $null }
    $selected = @()
    foreach ($name in @($version, "$version-SLInit")) {
        $section = Ini-Section $text $name
        $before = $text.Substring(0, $section.Index).TrimEnd("`r", "`n")
        $previous = ($before -split "`n")[-1].TrimEnd("`r")
        if ($previous -cne "; sha256=$hash") { return $null }
        $selected += $section.Value.TrimEnd("`r", "`n")
    }
    return ($selected -join "`r`n")
}
function Join-RdpWrapIni($community, $offsets) {
    # rdpwrap.dll lit mal les fins de ligne LF : tout le fichier doit rester en CRLF,
    # sinon TermService plante en boucle (c0000005 dans rdpwrap.dll).
    $ini = $community
    if ($offsets) { $ini += "`r`n" + $offsets + "`r`n" }
    return ($ini -replace "`r?`n", "`r`n")
}
function Restart-TermService {
    # Les places ouvertes sont déconnectées : l'utilisateur en est averti avant de confirmer.
    Stop-Service TermService -Force
    (Get-Service TermService).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
    Start-Service TermService
    (Get-Service TermService).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
    Start-Service UmRdpService -ErrorAction SilentlyContinue
}
function Hidden-Run($exe, $arguments) {
    $proc = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    return $proc.ExitCode
}
function Update-State([bool]$present) {
    if (-not $UpdateState -or -not (Test-Path -LiteralPath $stateFile)) { return }
    $state = Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json
    $list = @($state.thirdParty | Where-Object { $_ -and $_ -ne 'rdpwrap' })
    if ($present) { $list += 'rdpwrap' }
    $state.thirdParty = $list
    [IO.File]::WriteAllText($stateFile, (ConvertTo-Json -InputObject $state -Depth 8), [Text.Encoding]::UTF8)
}
function Add-Exclusion($path) {
    # Microsoft Defender classe RDP Wrapper comme outil à risque : sans exclusion il le supprime.
    # Seuls les dossiers de RDP Wrapper sont exclus, et uniquement sur accord explicite.
    try {
        $existing = @((Get-MpPreference).ExclusionPath)
        if ($existing -contains $path) { return $false }
        Add-MpPreference -ExclusionPath $path
        return $true
    } catch { Emit 'defender' 'warning' "Exclusion Microsoft Defender impossible : $($_.Exception.Message)" 15; return $false }
}
function Remove-Exclusion($path) { try { Remove-MpPreference -ExclusionPath $path } catch { } }

try {
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Droits administrateur requis' }

    if ($Action -eq 'Enable') {
        Emit 'check' 'running' 'Vérification de Windows' 5
        $dll = Service-Dll
        if (-not (Is-Stock $dll) -and -not (Test-Path -LiteralPath $marker)) {
            # RDP Wrapper ou TermWrap installé par l'utilisateur : on n'y touche pas.
            Emit 'complete' 'ok' 'Plusieurs sessions déjà actives (composant installé hors Kinseat)' 100
            exit 0
        }
        $version = Termsrv-Version
        # Déjà installé par Kinseat : réactiver = réparer (ini à jour après une mise à jour Windows).
        $already = (-not (Is-Stock $dll)) -and (Test-Path -LiteralPath $marker) -and (Test-Path -LiteralPath (Join-Path $rdp 'rdpwrap.dll'))
        if ($already) {
            $prior = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
            $owned = [ordered]@{ schema = 1; installedUtc = $prior.installedUtc; termsrv = $version; exclusions = @($prior.exclusions | Where-Object { $_ }) }
        } else {
            $owned = [ordered]@{ schema = 1; installedUtc = [DateTime]::UtcNow.ToString('o'); termsrv = $version; exclusions = @() }
        }

        Emit 'defender' 'running' 'Exclusion Microsoft Defender pour RDP Wrapper' 15
        if ($owned.exclusions -contains $work) {
            Remove-Exclusion $work
            $owned.exclusions = @($owned.exclusions | Where-Object { $_ -ne $work })
        }
        if (Add-Exclusion $rdp) { $owned.exclusions += $rdp }

        Emit 'download' 'running' 'Téléchargement de RDP Wrapper' 25
        if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
        New-Item -ItemType Directory -Path $work -Force | Out-Null
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        foreach ($key in @($(if (-not $already) { 'wrapper' }), 'ini') | Where-Object { $_ }) {
            $src = $sources[$key]; $target = Join-Path $work $src.file
            Invoke-WebRequest -Uri $src.url -OutFile $target -UseBasicParsing
            if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $src.sha) { throw "Fichier téléchargé corrompu ou modifié : $($src.file)" }
        }
        if (-not $already) {
            Expand-Archive -LiteralPath (Join-Path $work 'rdpwrap.zip') -DestinationPath (Join-Path $work 'rdpwrap') -Force
            $inst = Join-Path $work 'rdpwrap\RDPWInst.exe'
            if (-not (Test-Path -LiteralPath $inst)) { throw 'RDPWInst.exe absent ou supprimé par l''antivirus' }
        }

        Emit 'offsets' 'running' "Compatibilité avec Windows (termsrv $version)" 40
        $ini = [IO.File]::ReadAllText((Join-Path $work 'rdpwrap.ini'))
        if (-not (Ini-Covers $ini $version)) {
            $offsetFile = Join-Path $PSScriptRoot 'rdpwrap-offsets.ini'
            $offsets = $null
            if (Test-Path -LiteralPath $offsetFile -PathType Leaf) {
                $delivered = [IO.File]::ReadAllText($offsetFile)
                $hash = (Get-FileHash -LiteralPath (Join-Path $env:SystemRoot 'System32\termsrv.dll') -Algorithm SHA256).Hash
                $offsets = Select-ValidatedOffsets $delivered $version $hash
            }
            if (-not $offsets) {
                throw "Cette version de Windows (termsrv $version) n'est pas encore prise en charge par Kinseat. Installez la prochaine mise à jour de Kinseat. Vos places existantes ne sont pas modifiées."
            }
            $ini = Join-RdpWrapIni $ini $offsets
        }
        $ini = $ini -replace "`r?`n", "`r`n"

        if (-not $already) {
            Emit 'install' 'running' 'Installation de RDP Wrapper (les places ouvertes seront déconnectées)' 60
            $code = Hidden-Run $inst '-i'
            if ($code -ne 0) { throw "RDPWInst a échoué (code $code)" }
            if (-not (Test-Path -LiteralPath $rdp)) { New-Item -ItemType Directory -Path $rdp -Force | Out-Null }
            # Garder l'installateur pour pouvoir retirer RDP Wrapper plus tard (Paramètres ou désinstallation).
            Copy-Item -LiteralPath $inst -Destination (Join-Path $rdp 'RDPWInst.exe') -Force
        }
        [IO.File]::WriteAllText($marker, (ConvertTo-Json -InputObject $owned -Depth 4), [Text.Encoding]::UTF8)

        Emit 'configure' 'running' 'Configuration et redémarrage du Bureau à distance' 75
        Stop-Service TermService -Force
        (Get-Service TermService).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(30))
        [IO.File]::WriteAllText((Join-Path $rdp 'rdpwrap.ini'), ($ini -replace "`r?`n", "`r`n"), [Text.Encoding]::ASCII)
        Start-Service TermService
        (Get-Service TermService).WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
        Start-Service UmRdpService -ErrorAction SilentlyContinue

        # RDPWInst ouvre le port 3389 au réseau : le Bureau à distance reste limité à ce PC.
        Get-NetFirewallRule -Enabled True -Direction Inbound -Action Allow -ErrorAction SilentlyContinue |
            Where-Object { @(($_ | Get-NetFirewallPortFilter -ErrorAction SilentlyContinue).LocalPort) -contains '3389' } |
            ForEach-Object { Set-NetFirewallRule -Name $_.Name -RemoteAddress '127.0.0.1','127.0.0.2' -ErrorAction SilentlyContinue }

        Emit 'verify' 'running' 'Vérification' 90
        $dll = Service-Dll
        if (Is-Stock $dll) { throw 'RDP Wrapper ne s''est pas activé (Windows utilise toujours termsrv.dll)' }
        if (-not (Test-Path -LiteralPath (Join-Path $rdp 'rdpwrap.dll'))) { throw 'rdpwrap.dll absent : supprimé par l''antivirus ?' }
        if (-not (Ini-Covers ([IO.File]::ReadAllText((Join-Path $rdp 'rdpwrap.ini'))) $version)) { throw 'Configuration RDP Wrapper incomplète' }
        if ((Get-Service TermService).Status -ne 'Running') { throw 'Le service Bureau à distance ne redémarre pas' }
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
        Update-State $true
        Emit 'complete' 'ok' 'Plusieurs sessions Windows activées' 100
        exit 0
    }

    # Disable
    Emit 'check' 'running' 'Vérification' 10
    $dll = Service-Dll
    if (-not (Test-Path -LiteralPath $marker)) {
        if (Is-Stock $dll) { Update-State $false; Emit 'complete' 'ok' 'Plusieurs sessions déjà désactivées' 100; exit 0 }
        throw 'Ce composant multi-session a été installé hors Kinseat : retirez-le avec son propre programme'
    }
    $owned = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
    $inst = Join-Path $rdp 'RDPWInst.exe'
    Emit 'uninstall' 'running' 'Retrait de RDP Wrapper (les places ouvertes seront déconnectées)' 40
    if (Test-Path -LiteralPath $inst) {
        $code = Hidden-Run $inst '-u'
        if ($code -ne 0) { Emit 'uninstall' 'warning' "RDPWInst -u : code $code" 50 }
    }
    if (-not (Is-Stock (Service-Dll))) {
        # Retour garanti à l'état Windows d'origine même si RDPWInst a été supprimé.
        Set-ItemProperty -LiteralPath $params -Name ServiceDll -Value '%SystemRoot%\System32\termsrv.dll' -Type ExpandString
        Restart-TermService
    }
    Emit 'cleanup' 'running' 'Nettoyage' 75
    Remove-Item -LiteralPath $rdp -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    foreach ($path in @($owned.exclusions)) { if ($path) { Remove-Exclusion $path } }
    if (-not (Is-Stock (Service-Dll))) { throw 'Windows utilise encore RDP Wrapper' }
    if ((Get-Service TermService).Status -ne 'Running') { Start-Service TermService }
    Update-State $false
    Emit 'complete' 'ok' 'Plusieurs sessions Windows désactivées' 100
    exit 0
} catch {
    Emit 'complete' 'error' $_.Exception.Message 100
    exit 1
}
