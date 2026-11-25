#Requires -Version 5.1
param([Parameter(Mandatory=$true)][string]$Plan,[Parameter(Mandatory=$true)][string]$Log,[Parameter(Mandatory=$true)][string]$Progress)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$root=$PSScriptRoot
$install=Join-Path $env:ProgramFiles 'Kinseat'
$data=Join-Path $env:ProgramData 'Kinseat'
$stateFile=Join-Path $data 'install-state.json'
$rule='Kinseat - bloquer RDP réseau'
$state=[ordered]@{ schema=1; status='applying'; multisessionError=$null; previous=@{}; done=@(); backup=$null; serviceExisted=$false; serviceRunning=$false; thirdParty=@(); firewallOwned=$false; defenderOwned=$false; filesReplaced=$false }
function Emit($step,$status,$message,$pct) {
    $row=[ordered]@{ step=$step; status=$status; message=$message; pct=$pct; utc=[DateTime]::UtcNow.ToString('o') }
    $bytes=[Text.Encoding]::UTF8.GetBytes((ConvertTo-Json -Compress -InputObject $row)+"`r`n")
    $stream=[IO.FileStream]::new($Progress,[IO.FileMode]::Append,[IO.FileAccess]::Write,[IO.FileShare]::Read)
    try { $stream.Write($bytes,0,$bytes.Length); $stream.Flush() } finally { $stream.Dispose() }
}
function Save-State { [IO.File]::WriteAllText($stateFile,(ConvertTo-Json -InputObject $state -Depth 8),[Text.Encoding]::UTF8) }
function Step($id,$pct,[scriptblock]$work) {
    $start=[DateTime]::UtcNow
    Emit $id 'running' "Commande: $id" $pct
    try {
        $state.done+= $id; Save-State
        & $work
        Save-State
        $duration=[int]([DateTime]::UtcNow-$start).TotalMilliseconds
        [IO.File]::AppendAllText($Log,"$($start.ToString('o')) | $id | exit=0 | durationMs=$duration`r`n")
        Emit $id 'ok' "Terminé ($duration ms)" $pct
    } catch {
        $duration=[int]([DateTime]::UtcNow-$start).TotalMilliseconds
        [IO.File]::AppendAllText($Log,"$($start.ToString('o')) | $id | exit=1 | durationMs=$duration | $($_.Exception.Message)`r`n")
        Emit $id 'error' $_.Exception.Message $pct
        throw
    }
}
function Check-File($item) {
    if ($item.file -notmatch '^[a-zA-Z0-9_.\-/]+$' -or $item.file -match '(^|[/\\])\.\.([/\\]|$)' -or $item.file -match '(^|[/\\])\.([/\\]|$)') { throw "Chemin payload interdit: $($item.file)" }
    if ($item.sha256 -notmatch '^[a-fA-F0-9]{64}$') { throw "Empreinte absente: $($item.id)" }
    $file=Join-Path $root $item.file
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Charge absente: $($item.id)" }
    if ((Get-Item -LiteralPath $file -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Lien interdit: $($item.file)" }
    if ((Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $item.sha256) { throw "SHA-256 incorrect: $($item.file)" }
}
function Run-Sc([string[]]$Arguments) {
    & sc.exe @Arguments | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "sc.exe $($Arguments[0]) code $LASTEXITCODE" }
}
function Start-KinseatService {
    # First start of fresh binaries can exceed the SCM 30 s timeout (antivirus scan): retry.
    $name='MultiSeatService'
    for($attempt=1;$attempt -le 3;$attempt++) {
        try { Start-Service $name -ErrorAction Stop; return }
        catch {
            $err=$_
            $svc=Get-Service $name -ErrorAction SilentlyContinue
            if($svc -and $svc.Status -eq 'Running'){ return }
            if($attempt -ge 3){ throw $err }
            if($svc){
                $deadline=(Get-Date).AddSeconds(30)
                while($svc.Status -eq 'StartPending' -and (Get-Date) -lt $deadline){ Start-Sleep -Seconds 1; $svc.Refresh() }
                if($svc.Status -eq 'Running'){ return }
            }
            Start-Sleep -Seconds 5
        }
    }
}
function Restore-Rdp {
    foreach ($entry in $state.previous.GetEnumerator()) {
        $parts=$entry.Key -split '\|',2
        $key=$parts[0]; $name=$parts[1]
        $now=(Get-ItemProperty -LiteralPath $key -Name $name -ErrorAction SilentlyContinue).$name
        if ($now -ne $entry.Value.after) { [IO.File]::AppendAllText($Log,"RDP conflit : $name non restauré`r`n"); continue }
        if ($entry.Value.existed) { Set-ItemProperty -LiteralPath $key -Name $name -Value $entry.Value.before -Type DWord }
        else { Remove-ItemProperty -LiteralPath $key -Name $name -ErrorAction SilentlyContinue }
    }
}
function Set-Rdp($key,$name,$value) {
    if (-not (Test-Path -LiteralPath $key)) { New-Item -Path $key -Force | Out-Null }
    $prop=(Get-ItemProperty -LiteralPath $key -Name $name -ErrorAction SilentlyContinue).PSObject.Properties[$name]
    $existed=($null -ne $prop)
    $old=if($existed){$prop.Value}else{$null}
    if (-not $state.previous.Contains("$key|$name")) { $state.previous["$key|$name"]=[ordered]@{ existed=$existed; before=$old; after=$value }; Save-State }
    Set-ItemProperty -LiteralPath $key -Name $name -Value $value -Type DWord
}
try {
    $principal=New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Administrateur requis' }
    if (-not [Environment]::Is64BitOperatingSystem -or [Environment]::OSVersion.Version.Major -lt 10) { throw 'Windows 10/11 x64 requis' }
    $p=Get-Content -LiteralPath $Plan -Raw | ConvertFrom-Json
    if ($p.options.install_dir -ne $install -or ($p.options.rdpwrap -and -not $p.options.rdp_consent)) { throw 'Plan invalide' }
    $manifest=Get-Content -LiteralPath (Join-Path $root 'manifest.json') -Raw | ConvertFrom-Json
    foreach ($item in $manifest.artifacts | Where-Object { $_.required }) { Check-File $item }
    foreach ($entry in $manifest.artifacts | Where-Object { $_.required }) {
        if ($entry.file -match '^service/' -and -not (Test-Path -LiteralPath (Join-Path $root $entry.file) -PathType Leaf)) { throw "Fichier service manquant : $($entry.file)" }
    }
    foreach ($file in @(Get-ChildItem -LiteralPath (Join-Path $root 'service') -File -Recurse)) {
        $relative='service/' + $file.FullName.Substring((Join-Path $root 'service').Length+1).Replace('\','/')
        if (-not @($manifest.artifacts | Where-Object { $_.file -eq $relative -and $_.required }).Count) { throw "Fichier service non authentifié : $relative" }
    }
    foreach ($scriptName in @('Install-Kinseat.ps1','Uninstall-Kinseat.ps1','MultiSession-Kinseat.ps1','rdpwrap-offsets.ini',
        'service/SteamIsolation/Kinseat.GameFiles.dll','service/SteamIsolation/Kinseat.GameFiles32.dll')) {
        if (-not @($manifest.artifacts | Where-Object { $_.file -eq $scriptName -and $_.required }).Count) { throw "Script non signé par manifeste: $scriptName" }
    }
    $runtime=Get-Content -LiteralPath (Join-Path $root 'service/Kinseat.Service.runtimeconfig.json') -Raw | ConvertFrom-Json
    if (-not $runtime.runtimeOptions.includedFrameworks -or -not (Test-Path (Join-Path $root 'service/hostfxr.dll'))) { throw 'Publish autonome incohérent' }
    if (Test-Path -LiteralPath $stateFile) {
        $script:priorStateRaw=Get-Content -LiteralPath $stateFile -Raw
        $prior=$script:priorStateRaw | ConvertFrom-Json
        # 'rolled-back' = an earlier attempt failed and was fully undone: the machine is in its prior
        # state, so a new attempt is safe. Anything else (interrupted, rollback failed) is not.
        # 'uninstalled' = désinstallation complète avec données conservées : RDP, pare-feu et composants
        # tiers ont déjà été restaurés, la réinstallation repart donc d'un état neuf.
        if ($prior.status -notin @('complete','rolled-back','uninstalled')) { throw "État antérieur incertain : $stateFile ; intervention manuelle" }
        if ($prior.status -ne 'uninstalled') {
            # Upgrades retain the ORIGINAL RDP baseline.
            foreach ($entry in $prior.previous.PSObject.Properties) { $state.previous[$entry.Name]=$entry.Value }
            $state.firewallOwned=[bool]$prior.firewallOwned
            $state.defenderOwned=[bool]$prior.defenderOwned
            $state.thirdParty=@($prior.thirdParty)
        }
    }
    New-Item -ItemType Directory -Path $data -Force | Out-Null
    Save-State
    Step 'service-stop' 10 {
        $svc=Get-Service MultiSeatService -ErrorAction SilentlyContinue
        $state.serviceExisted=($null -ne $svc)
        $state.serviceRunning=($svc -and $svc.Status -eq 'Running')
        Save-State
        if ($state.serviceRunning) { Stop-Service MultiSeatService -Force; $svc.WaitForStatus('Stopped',[TimeSpan]::FromSeconds(40)) }
        # Seat agents and service helpers run from the install folder in other sessions and outlive the
        # service: they keep its DLLs locked. The service starts them again when it comes back.
        if (Test-Path $install) {
            $prefix=(Resolve-Path -LiteralPath $install).Path.TrimEnd('\') + '\'
            $deadline=(Get-Date).AddSeconds(15)
            do {
                $left=@(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) })
                $left | Stop-Process -Force -ErrorAction SilentlyContinue
                if ($left.Count) { Start-Sleep -Milliseconds 500 }
            } while ($left.Count -and (Get-Date) -lt $deadline)
            if ($left.Count) { throw ('Processus Kinseat encore actifs : ' + (($left | ForEach-Object { $_.Name + ' ' + $_.Id }) -join ', ')) }
        }
    }
    Step 'files' 25 {
        $stage=Join-Path $env:ProgramFiles ('Kinseat-stage-'+[guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $stage -Force | Out-Null
        try {
            Get-ChildItem -LiteralPath (Join-Path $root 'service') -Force | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $stage -Recurse -Force }
            foreach($name in @('kinseat.exe','kinseat-agent.exe','MultiSession-Kinseat.ps1','rdpwrap-offsets.ini')) { Copy-Item -LiteralPath (Join-Path $root $name) -Destination $stage -Force }
            Copy-Item -LiteralPath (Join-Path (Split-Path $root -Parent) 'setup.exe') -Destination $stage -Force
            # Setup en un seul .exe : il contient déjà la charge utile, inutile de la recopier.
            if (-not $env:KINSEAT_PAYLOAD_EMBEDDED) { Copy-Item -LiteralPath $root -Destination (Join-Path $stage 'payload') -Recurse -Force }
            if (-not (Test-Path (Join-Path $stage 'Kinseat.Service.exe')) -or -not (Test-Path (Join-Path $stage 'wwwroot/index.html'))) { throw 'Stage incomplet' }
            if (Test-Path $install) {
                $state.backup=Join-Path $data ('install-backup-'+[guid]::NewGuid().ToString('N'))
                Save-State
                Copy-Item -LiteralPath $install -Destination $state.backup -Recurse -Force
                # Seul appsettings.local.json porte les réglages de l'utilisateur (le service n'écrit que là) ;
                # garder l'ancien appsettings.json figerait les anciens défauts (ex. isolation des manettes).
                foreach($cfg in @('appsettings.local.json')) { if (Test-Path (Join-Path $install $cfg)) { Copy-Item -LiteralPath (Join-Path $install $cfg) -Destination $stage -Force } }
                Remove-Item -LiteralPath $install -Recurse -Force
            }
            $state.filesReplaced=$true; Save-State
            Move-Item -LiteralPath $stage -Destination $install
            if ((Get-FileHash (Join-Path $install 'Kinseat.Service.exe') -Algorithm SHA256).Hash -ne ($manifest.artifacts | Where-Object id -eq 'service').sha256) { throw 'Copie service altérée' }
            foreach($id in @('gamefiles64','gamefiles32')) {
                $item=$manifest.artifacts | Where-Object id -eq $id | Select-Object -First 1
                if (-not $item -or (Get-FileHash (Join-Path $install ($item.file -replace '^service/','')) -Algorithm SHA256).Hash -ne $item.sha256) { throw "Copie $id altérée" }
            }
        } finally { if (Test-Path $stage) { Remove-Item -LiteralPath $stage -Recurse -Force } }
    }
    Step 'rdp' 40 {
        $ts='HKLM:\SYSTEM\CurrentControlSet\Control\Terminal Server'
        $tcp='HKLM:\SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp'
        $policy='HKLM:\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services'
        Set-Rdp $ts 'fDenyTSConnections' 0
        Set-Rdp $tcp 'UserAuthentication' 0
        Set-Rdp $tcp 'SecurityLayer' 2
        Set-Rdp $policy 'AuthenticationLevel' 0
        Set-Rdp $policy 'AllowUnsignedFiles' 1
        Set-Rdp $policy 'fDisableAudioCapture' 0
        # Trust entries in HKCU belong to the console user, not elevated installer/SYSTEM.
    }
    Step 'firewall' 50 {
        $existing=Get-NetFirewallRule -DisplayName $rule -ErrorAction SilentlyContinue
        if ($p.options.keep_network_rdp) {
            if ($existing -and $state.firewallOwned) { $existing | Remove-NetFirewallRule; $state.firewallOwned=$false }
        } else {
            if ($existing -and -not $state.firewallOwned) {
                # Same name and exactly Kinseat's shape (inbound block of port 3389, nothing else):
                # this is Kinseat's own rule whose ownership record was lost (e.g. by an older
                # installer's rollback). Adopt it; refuse anything else.
                $ours=@($existing | Where-Object {
                    $_.Direction -eq 'Inbound' -and $_.Action -eq 'Block' -and
                    (@(($_ | Get-NetFirewallPortFilter).LocalPort) -join ',') -eq '3389' -and
                    ($_ | Get-NetFirewallApplicationFilter).Program -eq 'Any' })
                if ($ours.Count -ne @($existing).Count) { throw 'Règle pare-feu préexistante non possédée' }
                $state.firewallOwned=$true
            }
            if (-not $existing) {
                foreach($proto in @('TCP','UDP')) {
                    New-NetFirewallRule -DisplayName $rule -Group 'Kinseat' -Direction Inbound -Action Block -Profile Any -Protocol $proto -LocalPort 3389 -RemoteAddress Any | Out-Null
                }
                $state.firewallOwned=$true
            }
        }
        Save-State
    }
    Step 'service' 65 {
        $exe=Join-Path $install 'Kinseat.Service.exe'
        if ($state.serviceExisted) { Run-Sc @('config','MultiSeatService','binPath=',('"'+$exe+'"'),'start=','auto') }
        else { Run-Sc @('create','MultiSeatService','binPath=',('"'+$exe+'"'),'start=','auto','DisplayName=','"MultiSeat Multi-Seat Streaming Service"') }
        Run-Sc @('description','MultiSeatService','Manages multi-seat headless game streaming sessions with isolated input, audio, and display.')
        Run-Sc @('failure','MultiSeatService','reset=','86400','actions=','restart/5000/restart/10000/restart/30000')
        Start-KinseatService
        (Get-Service MultiSeatService).WaitForStatus('Running',[TimeSpan]::FromSeconds(60))
        $pipe='\\.\pipe\Kinseat.Admin.v1'
        $ready=$false
        for($i=0;$i -lt 60;$i++) { if(Test-Path -LiteralPath $pipe){$ready=$true;break}; Start-Sleep -Seconds 1 }
        if(-not $ready){throw "Pipe admin non prêt : $pipe"}
    }
    Step 'optionals' 80 {
        foreach($entry in @(@('apollo',$p.options.apollo),@('vdd',$p.options.virtual_display))) {
            $id=$entry[0]; if(-not $entry[1]) { continue }
            $item=$manifest.artifacts | Where-Object id -eq $id | Select-Object -First 1
            try { if(-not $item){ throw 'absent' }; Check-File $item }
            catch { Emit $id 'skipped' "$id ignoré (archive absente ou SHA-256 non qualifié)" 80; continue }
            if ($id -eq 'apollo') {
                $dest=Join-Path $env:ProgramFiles 'ApolloVibe'
                $marker=Join-Path $dest 'kinseat-apollo.sha256'
                # Complet = exécutable + shaders. Un dossier partiel (ex. suppression interrompue par un
                # sunshine.exe verrouillé) n'est pas une installation existante : il est réparé.
                $complete=(Test-Path (Join-Path $dest 'sunshine.exe')) -and
                    @(Get-ChildItem -LiteralPath (Join-Path $dest 'assets\shaders') -Recurse -File -ErrorAction SilentlyContinue).Count -gt 0
                $ours=Test-Path -LiteralPath $marker
                $stale=$ours -and ((Get-Content -LiteralPath $marker -Raw).Trim() -ne $item.sha256)
                if (-not $complete -or $stale) {
                    Get-Process sunshine -ErrorAction SilentlyContinue |
                        Where-Object { $_.Path -and $_.Path.StartsWith($dest, [StringComparison]::OrdinalIgnoreCase) } |
                        Stop-Process -Force -ErrorAction SilentlyContinue
                    Start-Sleep -Milliseconds 500
                    New-Item -ItemType Directory -Path $dest -Force | Out-Null
                    Expand-Archive -LiteralPath (Join-Path $root $item.file) -DestinationPath $dest -Force
                    Set-Content -LiteralPath $marker -Value $item.sha256 -Encoding ascii
                    if ($state.thirdParty -notcontains 'apollo') { $state.thirdParty+= 'apollo' }; Save-State
                    Emit $id 'ok' $(if ($complete) { 'ApolloVibe mis à jour' } else { 'ApolloVibe installé ou réparé' }) 80
                } else { Emit $id 'skipped' 'ApolloVibe déjà installé et complet' 80 }
            } else {
                $work=Join-Path $data ('vdd-'+[guid]::NewGuid().ToString('N'))
                Expand-Archive -LiteralPath (Join-Path $root $item.file) -DestinationPath $work
                try {
                    $inf=Get-ChildItem -LiteralPath $work -Filter '*.inf' -Recurse -File | Select-Object -First 1
                    if (-not $inf) { throw 'INF SudoVDA absent' }
                    & pnputil.exe /add-driver $inf.FullName /install | Out-Null
                    if ($LASTEXITCODE -ne 0) { throw "pnputil code $LASTEXITCODE" }
                    $state.thirdParty+= 'vdd'; Save-State
                } finally { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
            }
        }
        # HidHide : isole la manette de chaque place (sinon la touche Guide d'un joueur ouvre la
        # Game Bar sur l'écran de l'hôte). Installé seulement s'il manque ; retiré à la
        # désinstallation seulement si Kinseat l'a installé.
        $hh=$manifest.artifacts | Where-Object id -eq 'hidhide' | Select-Object -First 1
        $hhCli=Join-Path $env:ProgramFiles 'Nefarius Software Solutions\HidHide\x64\HidHideCLI.exe'
        if ($p.options.apollo -and $hh) {
            if (Test-Path -LiteralPath $hhCli) { Emit 'hidhide' 'skipped' 'HidHide déjà installé' 80 }
            else {
                try {
                    Check-File $hh
                    # Archive qualifiée : MSI officiel HidHide 1.5.230 dont seules les actions
                    # « updater » (Register/DeregisterUpdater) sont retirées ; elles ne se terminent
                    # jamais hors session interactive (mesuré sur VM). Pilote signé inchangé.
                    $hhWork=Join-Path $env:SystemRoot ('Temp\kinseat-hidhide-'+[guid]::NewGuid().ToString('N'))
                    Expand-Archive -LiteralPath (Join-Path $root $hh.file) -DestinationPath $hhWork -Force
                    $hhMsi=Join-Path $hhWork 'HidHide.msi'
                    if (-not (Test-Path -LiteralPath $hhMsi)) { throw 'HidHide.msi absent de l''archive' }
                    $hhLog=Join-Path (Split-Path $Log -Parent) 'hidhide-msi.log'
                    $proc=Start-Process msiexec.exe -ArgumentList "/i `"$hhMsi`" /qn /norestart /l*v `"$hhLog`"" -WindowStyle Hidden -PassThru
                    if (-not $proc.WaitForExit(360000)) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; throw 'délai dépassé' }
                    Remove-Item -LiteralPath $hhWork -Recurse -Force -ErrorAction SilentlyContinue
                    if ($proc.ExitCode -notin 0,3010) { throw "msiexec code $($proc.ExitCode), journal $hhLog" }
                    if (-not (Test-Path -LiteralPath $hhCli)) { throw 'HidHideCLI absent après installation' }
                    if ($state.thirdParty -notcontains 'hidhide') { $state.thirdParty+= 'hidhide' }; Save-State
                    Emit 'hidhide' $(if ($proc.ExitCode -eq 3010) { 'warning' } else { 'ok' }) $(if ($proc.ExitCode -eq 3010) { 'HidHide installé : redémarrez le PC pour isoler les manettes' } else { 'HidHide installé (manettes isolées par place)' }) 80
                } catch { Emit 'hidhide' 'warning' "HidHide non installé ($($_.Exception.Message)) : les manettes des places restent visibles sur l'hôte" 80 }
            }
        }
        if($p.options.apollo -and $state.thirdParty -notcontains 'apollo' -and -not (Test-Path (Join-Path $env:ProgramFiles 'ApolloVibe/sunshine.exe'))) { Emit 'apollo' 'warning' 'Streaming indisponible tant qu''Apollo n''est pas installé' 80 }
        if($p.options.defender_exclusion) { Add-MpPreference -ExclusionPath $install; $state.defenderOwned=$true; Save-State }
    }
    if ($p.options.rdpwrap) {
        # Plusieurs sessions Windows (RDP Wrapper) : uniquement sur case cochée par l'utilisateur.
        # Non bloquant : Kinseat reste installé ; l'option se réessaie depuis Paramètres.
        $start=[DateTime]::UtcNow
        Emit 'multisession' 'running' 'Activation de plusieurs sessions Windows (RDP Wrapper)' 88
        $msProgress=Join-Path $data 'multisession.jsonl'
        $msLog=Join-Path (Split-Path $Log -Parent) 'multisession.log'
        $proc=Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-WindowStyle','Hidden','-File',('"'+(Join-Path $install 'MultiSession-Kinseat.ps1')+'"'),'-Action','Enable','-Progress',('"'+$msProgress+'"'),'-Log',('"'+$msLog+'"')) -WindowStyle Hidden -Wait -PassThru
        $last=@(Get-Content -LiteralPath $msProgress -Encoding UTF8 -ErrorAction SilentlyContinue | Where-Object { $_ } | ForEach-Object { try { $_ | ConvertFrom-Json } catch { } }) | Select-Object -Last 1
        $msg=if ($last) { $last.message } else { "code $($proc.ExitCode)" }
        $duration=[int]([DateTime]::UtcNow-$start).TotalMilliseconds
        [IO.File]::AppendAllText($Log,"$($start.ToString('o')) | multisession | exit=$($proc.ExitCode) | durationMs=$duration`r`n")
        if ($proc.ExitCode -eq 0) {
            if ($state.thirdParty -notcontains 'rdpwrap') { $state.thirdParty+= 'rdpwrap' }; Save-State
            Emit 'multisession' 'ok' $msg 88
        } else {
            # Le setup lit install-state.json à la fin : exposer l'échec optionnel en clair.
            $state.multisessionError=$msg; Save-State
            Emit 'multisession' 'warning' "Plusieurs sessions non activées : $msg. Réessayez depuis Paramètres." 88
        }
    }
    Step 'registration' 95 {
        $shell=New-Object -ComObject WScript.Shell
        $start=Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\Kinseat Hôte.lnk'
        $hostExe=Join-Path $install 'kinseat.exe'
        $link=$shell.CreateShortcut($start); $link.TargetPath=$hostExe; $link.WorkingDirectory=$install; $link.IconLocation=$hostExe+',0'; $link.Save()
        $desk=Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Kinseat Hôte.lnk'
        $link=$shell.CreateShortcut($desk); $link.TargetPath=$hostExe; $link.WorkingDirectory=$install; $link.IconLocation=$hostExe+',0'; $link.Save()
        $uninstall='HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Kinseat'
        New-Item $uninstall -Force | Out-Null
        $values=@{DisplayName='Kinseat Hôte';DisplayVersion='0.3.21';Publisher='Kinseat';DisplayIcon=((Join-Path $install 'kinseat.exe')+',0');InstallLocation=$install;UninstallString=('"'+(Join-Path $install 'setup.exe')+'" --uninstall')}
        foreach($name in $values.Keys){Set-ItemProperty $uninstall -Name $name -Value $values[$name]}
        foreach($name in @('NoModify','NoRepair')){Set-ItemProperty $uninstall -Name $name -Value 1 -Type DWord}
        $size=[int][Math]::Ceiling(((@(Get-ChildItem $install -File -Recurse | Measure-Object Length -Sum).Sum)/1KB))
        Set-ItemProperty $uninstall -Name EstimatedSize -Value $size -Type DWord
    }
    $state.status='complete'; Save-State
    Emit 'complete' 'ok' 'Installation terminée ; vérifier les composants optionnels' 100
} catch {
    $failure=$_.Exception.Message
    [IO.File]::AppendAllText($Log,"FAILURE $failure ; rollback inverse`r`n")
    Emit 'rollback' 'running' $failure 0
    try {
        $completed=@($state.done)
        [array]::Reverse($completed)
        foreach($id in $completed) {
            switch($id) {
                'registration' {
                    Remove-Item 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Kinseat' -Recurse -Force -ErrorAction SilentlyContinue
                    Remove-Item (Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\Kinseat Hôte.lnk') -Force -ErrorAction SilentlyContinue
                    Remove-Item (Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Kinseat Hôte.lnk') -Force -ErrorAction SilentlyContinue
                }
                'service' { Stop-Service MultiSeatService -Force -ErrorAction SilentlyContinue; if(-not $state.serviceExisted){ & sc.exe delete MultiSeatService | Out-Null } }
                'firewall' { if($state.firewallOwned){Get-NetFirewallRule -DisplayName $rule -ErrorAction SilentlyContinue | Remove-NetFirewallRule} }
                'rdp' { Restore-Rdp }
                'files' { if($state.filesReplaced){if(Test-Path $install){Remove-Item -LiteralPath $install -Recurse -Force}; if($state.backup -and (Test-Path $state.backup)){Move-Item -LiteralPath $state.backup -Destination $install}} }
            }
        }
        if($state.serviceRunning){Start-Service MultiSeatService}
        if($state.defenderOwned){Remove-MpPreference -ExclusionPath $install}
        # Put the previous installation's record back (with its original RDP/firewall/third-party
        # baseline): the failed attempt must not erase it nor block the next update.
        if ($script:priorStateRaw) {
            [IO.File]::WriteAllText($stateFile, $script:priorStateRaw, (New-Object Text.UTF8Encoding($false)))
        } else { $state.status='rolled-back'; Save-State }
    } catch { $state.status='intervention-required'; Save-State; [IO.File]::AppendAllText($Log,"ROLLBACK FAILED $($_.Exception.Message)`r`n") }
    Emit 'rollback' 'error' "$failure ; état $($state.status)" 0
    exit 1
}
