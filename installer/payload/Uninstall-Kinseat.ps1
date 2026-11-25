#Requires -Version 5.1
param([switch]$KeepData,[Parameter(Mandatory=$true)][string]$Log,[Parameter(Mandatory=$true)][string]$Progress)
$ErrorActionPreference='Stop'
$install=Join-Path $env:ProgramFiles 'Kinseat'
$data=Join-Path $env:ProgramData 'Kinseat'
$stateFile=Join-Path $data 'install-state.json'
function Emit($step,$status,$message,$pct) {
    $row=@{step=$step;status=$status;message=$message;pct=$pct;utc=[DateTime]::UtcNow.ToString('o')}
    $bytes=[Text.Encoding]::UTF8.GetBytes((ConvertTo-Json -Compress -InputObject $row)+"`r`n")
    $stream=[IO.FileStream]::new($Progress,[IO.FileMode]::Append,[IO.FileAccess]::Write,[IO.FileShare]::Read)
    try { $stream.Write($bytes,0,$bytes.Length); $stream.Flush() } finally { $stream.Dispose() }
    [IO.File]::AppendAllText($Log,"$([DateTime]::UtcNow.ToString('o')) | $step | $status | $message`r`n")
}
try {
    $principal=New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if(-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Administrateur requis'}
    if(-not (Test-Path $stateFile)){throw 'État d''installation absent : désinstallation conservatrice refusée'}
    $state=Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json
    if($state.schema -ne 1 -or $state.status -ne 'complete'){throw 'État incertain : intervention manuelle requise'}
    Emit 'service' 'running' 'Arrêt et restauration des droits avant suppression du service' 10
    $svc=Get-Service MultiSeatService -ErrorAction SilentlyContinue
    if($svc -and $svc.Status -ne 'Stopped'){Stop-Service MultiSeatService -Force; $svc.WaitForStatus('Stopped',[TimeSpan]::FromSeconds(40))}
    # Import only USER_RIGHTS; preserve every other exported privilege and unrelated SID.
    $rightsState=Join-Path $data 'shutdown-rights\state.json'
    if(Test-Path -LiteralPath $rightsState){
        $snapshot=Get-Content -LiteralPath $rightsState -Raw | ConvertFrom-Json
        $expected=@('SeShutdownPrivilege','SeRemoteShutdownPrivilege')
        if(@($snapshot.Changes).Count -ne 2 -or
            ((@($snapshot.Changes | ForEach-Object Right | Sort-Object) -join ',') -ne
             (@($expected | Sort-Object) -join ','))){
            throw 'Sauvegarde LSA invalide : désinstallation refusée'
        }
        foreach($change in $snapshot.Changes){
            if(@($change.Remove | Where-Object {$_ -notin @('S-1-1-0','S-1-5-4','S-1-5-11','S-1-5-14','S-1-5-32-545')}).Count -or
               @($change.Add | Where-Object {$_ -ne 'S-1-5-32-544' -and $_ -notmatch '^S-1-5-21-(\d+-){3}\d+$'}).Count){
                throw 'Sauvegarde LSA contenant un SID inattendu : désinstallation refusée'
            }
        }
        $tmpRights=Join-Path $env:TEMP ('kinseat-rights-'+[guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $tmpRights | Out-Null
        try {
            $export=Join-Path $tmpRights 'current.inf'
            & secedit.exe /export /areas USER_RIGHTS /cfg $export | Out-Null
            if($LASTEXITCODE -ne 0){throw "Export LSA impossible : $LASTEXITCODE"}
            $lines=@(Get-Content -LiteralPath $export -Encoding Unicode)
            foreach($change in $snapshot.Changes){
                $match=@(for($i=0;$i -lt $lines.Count;$i++){if($lines[$i] -match ('^'+$change.Right+'\s*=')){$i}})
                if($match.Count -gt 1){throw "Attribution dupliquée : $($change.Right)"}
                $current=if($match.Count){@(($lines[$match[0]] -split '=',2)[1] -split ',' | ForEach-Object {$_.Trim().TrimStart('*')} | Where-Object {$_})}else{@()}
                $restore=@($current | Where-Object {$_ -notin @($change.Add)})
                $restore+=@($change.Remove | Where-Object {$_ -notin $restore})
                $entry=$change.Right+' = '+(($restore | ForEach-Object {'*'+$_}) -join ',')
                if($match.Count){$lines[$match[0]]=$entry}else{$lines+= $entry}
            }
            [IO.File]::WriteAllLines($export,$lines,[Text.Encoding]::Unicode)
            & secedit.exe /configure /db (Join-Path $tmpRights 'restore.sdb') /cfg $export /areas USER_RIGHTS /quiet | Out-Null
            if($LASTEXITCODE -ne 0){throw "Restauration LSA impossible : $LASTEXITCODE"}
            $verify=Join-Path $tmpRights 'verify.inf'
            & secedit.exe /export /areas USER_RIGHTS /cfg $verify | Out-Null
            if($LASTEXITCODE -ne 0){throw "Vérification LSA impossible : $LASTEXITCODE"}
            $actual=@(Get-Content -LiteralPath $verify -Encoding Unicode)
            foreach($change in $snapshot.Changes){
                $line=@($actual | Where-Object {$_ -match ('^'+$change.Right+'\s*=')})
                if($line.Count -ne 1){throw "Droit LSA non vérifiable : $($change.Right)"}
                $ids=@(($line[0] -split '=',2)[1] -split ',' | ForEach-Object {$_.Trim().TrimStart('*')})
                if(@($change.Remove | Where-Object {$_ -notin $ids}).Count){throw "Droit LSA non restauré : $($change.Right)"}
            }
            $groupName = if ($snapshot.GroupName) { [string]$snapshot.GroupName } else { 'Kinseat-ArretAutorise' }
            if ($groupName -eq 'Kinseat-ArretAutorise') {
                $grp = Get-LocalGroup -Name $groupName -ErrorAction SilentlyContinue
                if ($grp) { Remove-LocalGroup -Name $groupName -ErrorAction SilentlyContinue }
            }
            Remove-Item -LiteralPath $rightsState -Force
            Emit 'service' 'ok' 'Droits LSA restaurés et groupe dédié supprimé' 20
        } finally {Remove-Item -LiteralPath $tmpRights -Recurse -Force -ErrorAction SilentlyContinue}
    }
    if($svc){& sc.exe delete MultiSeatService | Out-Null; if($LASTEXITCODE -ne 0){throw "sc delete code $LASTEXITCODE"}}
    Emit 'service' 'ok' 'Service supprimé' 25
    Get-NetFirewallRule -DisplayName 'Kinseat-Join' -ErrorAction SilentlyContinue | Remove-NetFirewallRule
    if($state.firewallOwned){Get-NetFirewallRule -DisplayName 'Kinseat - bloquer RDP réseau' -ErrorAction SilentlyContinue | Remove-NetFirewallRule}
    foreach($entry in $state.previous.PSObject.Properties){
        $parts=$entry.Name -split '\|',2
        $now=(Get-ItemProperty -LiteralPath $parts[0] -Name $parts[1] -ErrorAction SilentlyContinue).($parts[1])
        if($now -ne $entry.Value.after){Emit 'rdp' 'warning' "Conflit : $($parts[1]) non restauré" 40; continue}
        if($entry.Value.existed){Set-ItemProperty -LiteralPath $parts[0] -Name $parts[1] -Value $entry.Value.before -Type DWord}
        else{Remove-ItemProperty -LiteralPath $parts[0] -Name $parts[1] -ErrorAction SilentlyContinue}
    }
    Emit 'rdp' 'ok' 'Valeurs antérieures restaurées si non modifiées depuis' 50
    if($state.defenderOwned){Remove-MpPreference -ExclusionPath $install}
    # Règles HidHide « !session » posées par le service : elles persistent dans le pilote.
    $hhCli=Join-Path $env:ProgramFiles 'Nefarius Software Solutions\HidHide\x64\HidHideCLI.exe'
    if(Test-Path -LiteralPath $hhCli){
        $tmp=Join-Path $env:TEMP ('hh-'+[guid]::NewGuid().ToString('N')+'.txt')
        & cmd.exe /c "`"$hhCli`" --dev-list > `"$tmp`" 2>&1"
        $jailed=@(Get-Content -LiteralPath $tmp -ErrorAction SilentlyContinue | ForEach-Object { if($_ -match '--dev-hide "([A-Za-z0-9\\&_!.\-{}:]+![0-9]+)"'){ $Matches[1] } })
        foreach($rule in $jailed){ & cmd.exe /c "`"$hhCli`" --dev-unhide `"$rule`" > nul 2>&1" }
        $remaining=@(Get-Content -LiteralPath $tmp -ErrorAction SilentlyContinue | Where-Object { $_ -match '--dev-hide' }).Count - $jailed.Count
        if($remaining -le 0){ & cmd.exe /c "`"$hhCli`" --cloak-off > nul 2>&1" }
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
        Emit 'hidhide' 'ok' "Règles d'isolation des manettes retirées ($($jailed.Count))" 55
    }
    # Plusieurs sessions (RDP Wrapper) : retiré seulement s'il a été installé par Kinseat (marqueur),
    # avant la suppression du dossier qui contient le script.
    $ms=Join-Path $install 'MultiSession-Kinseat.ps1'
    if((Test-Path -LiteralPath $ms) -and (Test-Path -LiteralPath (Join-Path $env:ProgramFiles 'RDP Wrapper\kinseat-owned.json'))){
        $msProgress=Join-Path $env:TEMP ('kinseat-ms-'+[guid]::NewGuid().ToString('N')+'.jsonl')
        $proc=Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe') -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-WindowStyle','Hidden','-File',('"'+$ms+'"'),'-Action','Disable','-Progress',('"'+$msProgress+'"'),'-Log',('"'+(Join-Path (Split-Path $Log -Parent) 'multisession.log')+'"')) -WindowStyle Hidden -Wait -PassThru
        if($proc.ExitCode -eq 0){Emit 'multisession' 'ok' 'Plusieurs sessions Windows désactivées (RDP Wrapper retiré)' 58}
        else{$last=@(Get-Content -LiteralPath $msProgress -Encoding UTF8 -ErrorAction SilentlyContinue | Where-Object { $_ } | ForEach-Object { try { $_ | ConvertFrom-Json } catch { } }) | Select-Object -Last 1; Emit 'multisession' 'warning' "RDP Wrapper non retiré : $(if($last){$last.message}else{$proc.ExitCode})" 58}
        Remove-Item -LiteralPath $msProgress -Force -ErrorAction SilentlyContinue
    }
    foreach($id in @($state.thirdParty)){
        if($id -eq 'apollo'){
            $apollo=Join-Path $env:ProgramFiles 'ApolloVibe'
            # Un sunshine.exe encore actif verrouille l'exécutable et laisserait un dossier partiel.
            Get-Process sunshine -ErrorAction SilentlyContinue |
                Where-Object { $_.Path -and $_.Path.StartsWith($apollo, [StringComparison]::OrdinalIgnoreCase) } |
                Stop-Process -Force -ErrorAction SilentlyContinue
            Start-Sleep -Milliseconds 500
            Remove-Item -LiteralPath $apollo -Recurse -Force -ErrorAction Stop
        }
        elseif($id -eq 'hidhide'){
            $hhKey=Get-ItemProperty 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
                Where-Object { $_.DisplayName -eq 'HidHide' -and $_.PSChildName -match '^\{[0-9A-Fa-f-]+\}$' } | Select-Object -First 1
            if($hhKey){
                $proc=Start-Process msiexec.exe -ArgumentList "/x $($hhKey.PSChildName) /qn /norestart" -WindowStyle Hidden -PassThru
                $deadline=(Get-Date).AddMinutes(6)
                while(-not $proc.HasExited -and (Get-Date) -lt $deadline){
                    Get-Process nefarius_HidHide_Updater -ErrorAction SilentlyContinue |
                        Where-Object { ((Get-Date)-$_.StartTime).TotalSeconds -gt 20 } | Stop-Process -Force -ErrorAction SilentlyContinue
                    Start-Sleep -Seconds 2
                }
                if(-not $proc.HasExited){Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue}
                if($proc.HasExited -and $proc.ExitCode -in 0,3010){Emit 'hidhide' 'ok' 'HidHide désinstallé' 60}
                else{Emit 'hidhide' 'warning' "HidHide : désinstallation code $($proc.ExitCode), retirer manuellement" 60}
            }
        }
        elseif($id -eq 'rdpwrap'){}
        elseif($id -eq 'vdd'){Emit 'vdd' 'warning' 'Pilote SudoVDA : désinstallation automatique non sûre ; retirer manuellement' 60}
    }
    $shortcuts=Join-Path $data 'shutdown-rights\shortcuts.json'
    if(Test-Path -LiteralPath $shortcuts){
        foreach($entry in @(Get-Content -LiteralPath $shortcuts -Raw | ConvertFrom-Json)){
            if($entry.Sid -notmatch '^S-1-5-21-(\d+-){3}\d+$' -or
                [IO.Path]::GetFileName($entry.Path) -notin @('Kinseat Hôte.lnk','HidHide Configurator.lnk')){
                throw 'Inventaire de raccourcis invalide : désinstallation refusée'
            }
            if(Test-Path -LiteralPath $entry.Path){
                $acl=Get-Acl -LiteralPath $entry.Path
                $rule=[Security.AccessControl.FileSystemAccessRule]::new(
                    [Security.Principal.SecurityIdentifier]::new($entry.Sid),
                    [Security.AccessControl.FileSystemRights]::Read,
                    [Security.AccessControl.AccessControlType]::Deny)
                $acl.RemoveAccessRuleSpecific($rule)
                Set-Acl -LiteralPath $entry.Path -AclObject $acl
            }
        }
        Remove-Item -LiteralPath $shortcuts -Force
    }
    Remove-Item 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Kinseat' -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\Kinseat Hôte.lnk') -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Kinseat Hôte.lnk') -Force -ErrorAction SilentlyContinue
    # setup.exe runs from this directory: Windows may hold the image open until it exits.
    if (Test-Path $install) {
        $prefix=(Resolve-Path -LiteralPath $install).Path.TrimEnd('\') + '\'
        # Seat agents and service helpers keep DLLs locked; never stop setup.exe, which runs this script.
        Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Name -ne 'setup' -and $_.Path.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase) } | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 800
    }
    try {Remove-Item -LiteralPath $install -Recurse -Force -ErrorAction Stop}
    catch {Emit 'files' 'warning' 'Fichiers encore verrouillés : suppression manuelle ou après redémarrage nécessaire' 85}
    if(-not $KeepData){Remove-Item -LiteralPath $data -Recurse -Force}
    else { $state.status='uninstalled'; [IO.File]::WriteAllText($stateFile,(ConvertTo-Json -InputObject $state -Depth 8)) }
    Emit 'complete' 'ok' 'Désinstallation terminée (voir avertissements)' 100
} catch {Emit 'failure' 'error' $_.Exception.Message 0; exit 1}
