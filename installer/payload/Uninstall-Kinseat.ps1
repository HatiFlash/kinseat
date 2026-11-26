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
    $state=Get-Content -LiteralPath $stateFile -Raw -Encoding UTF8 | ConvertFrom-Json
    if($state.schema -ne 1 -or $state.status -ne 'complete'){throw 'État incertain : intervention manuelle requise'}
    Emit 'service' 'running' 'Arrêt et restauration des droits avant suppression du service' 10
    $svc=Get-Service MultiSeatService -ErrorAction SilentlyContinue
    if($svc -and $svc.Status -ne 'Stopped'){Stop-Service MultiSeatService -Force; $svc.WaitForStatus('Stopped',[TimeSpan]::FromSeconds(40))}
    function Get-UserRightEntries($lines, $right) {
        $match = @(for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match ('^' + $right + '\s*=')) { $i } })
        if ($match.Count -gt 1) { throw "Attribution dupliquée : $right" }
        if ($match.Count -eq 0) { return @() }
        return @(($lines[$match[0]] -split '=', 2)[1] -split ',' | ForEach-Object { $_.Trim().TrimStart('*') } | Where-Object { $_ })
    }
    function Restore-ShutdownRightsInf($tmpRights, $targetRights) {
        $restoreInf = Join-Path $tmpRights 'restore.inf'
        $infLines = @(
            '[Unicode]'
            'Unicode=yes'
            '[Version]'
            'signature="$CHICAGO$"'
            'Revision=1'
            '[Privilege Rights]'
        )
        foreach ($right in @('SeShutdownPrivilege', 'SeRemoteShutdownPrivilege')) {
            $formatted = @($targetRights[$right] | ForEach-Object { if ($_ -match '^S-\d+-') { '*' + $_ } else { $_ } })
            $infLines += ($right + ' = ' + ($formatted -join ','))
        }
        [IO.File]::WriteAllLines($restoreInf, $infLines, [Text.Encoding]::Unicode)
        & secedit.exe /configure /db (Join-Path $tmpRights 'restore.sdb') /cfg $restoreInf /areas USER_RIGHTS /quiet | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Restauration LSA impossible : $LASTEXITCODE" }
        $verify = Join-Path $tmpRights 'verify.inf'
        & secedit.exe /export /areas USER_RIGHTS /cfg $verify | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Vérification LSA impossible : $LASTEXITCODE" }
        $actualLines = @(Get-Content -LiteralPath $verify -Encoding Unicode)
        foreach ($right in @('SeShutdownPrivilege', 'SeRemoteShutdownPrivilege')) {
            $expectedIds = @($targetRights[$right])
            $actualIds = @(Get-UserRightEntries $actualLines $right)
            $missing = @($expectedIds | Where-Object { $_ -notin $actualIds })
            $extra = @($actualIds | Where-Object { $_ -notin $expectedIds })
            if ($missing.Count -gt 0 -or $extra.Count -gt 0) {
                throw "Droit LSA non restauré conformément à l'attendu : $right"
            }
        }
    }
    # Import a minimal USER_RIGHTS INF containing only the two shutdown rights; preserve all other rights.
    $expected=@('SeShutdownPrivilege','SeRemoteShutdownPrivilege')
    $rightsState=Join-Path $data 'shutdown-rights\state.json'
    if(Test-Path -LiteralPath $rightsState){
        $snapshot=Get-Content -LiteralPath $rightsState -Raw -Encoding UTF8 | ConvertFrom-Json
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
        if($snapshot.GroupName -and $snapshot.GroupName -ne 'Kinseat-ArretAutorise'){
            throw 'Nom du groupe LSA inattendu : désinstallation refusée'
        }
        if($snapshot.GroupSid -and $snapshot.GroupSid -notmatch '^S-1-5-21-(\d+-){3}\d+$'){
            throw 'SID du groupe LSA invalide : désinstallation refusée'
        }
        $groupName='Kinseat-ArretAutorise'
        $grp=Get-LocalGroup -Name $groupName -ErrorAction SilentlyContinue
        $grpSid=if($grp -and $grp.SID){$grp.SID.Value}else{$null}
        if($grpSid -and $snapshot.GroupSid -and $grpSid -ne $snapshot.GroupSid){
            throw 'SID du groupe LSA différent de la sauvegarde : désinstallation refusée'
        }
        $dedicatedSid=if($snapshot.GroupSid){[string]$snapshot.GroupSid}else{$grpSid}
        $tmpRights=Join-Path $env:TEMP ('kinseat-rights-'+[guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $tmpRights | Out-Null
        try {
            $export=Join-Path $tmpRights 'current.inf'
            & secedit.exe /export /areas USER_RIGHTS /cfg $export | Out-Null
            if($LASTEXITCODE -ne 0){throw "Export LSA impossible : $LASTEXITCODE"}
            $lines=@(Get-Content -LiteralPath $export -Encoding Unicode)
            $targetRights=@{}
            foreach($change in $snapshot.Changes){
                $current=@(Get-UserRightEntries $lines $change.Right)
                # secedit peut exporter ce groupe sous son nom, tandis que le journal contient son SID.
                $restore=@($current | Where-Object {
                    $_ -notin @($change.Add) -and
                    (-not $dedicatedSid -or $_ -ne $dedicatedSid) -and
                    $_ -ne $groupName -and $_ -notlike "*\$groupName"
                })
                $restore+=@($change.Remove | Where-Object {$_ -notin $restore})
                $targetRights[$change.Right]=$restore
            }
            Restore-ShutdownRightsInf $tmpRights $targetRights
            if($grp){Remove-LocalGroup -Name $groupName -ErrorAction Stop}
            Remove-Item -LiteralPath $rightsState -Force
            Emit 'service' 'ok' 'Droits LSA restaurés et groupe dédié supprimé' 20
        } finally {Remove-Item -LiteralPath $tmpRights -Recurse -Force -ErrorAction SilentlyContinue}
    } else {
        $groupName = 'Kinseat-ArretAutorise'
        $grp = Get-LocalGroup -Name $groupName -ErrorAction SilentlyContinue
        $grpSid = if ($grp -and $grp.SID) { $grp.SID.Value } else { $null }
        $tmpRights=Join-Path $env:TEMP ('kinseat-rights-'+[guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $tmpRights | Out-Null
        try {
            $export=Join-Path $tmpRights 'current.inf'
            & secedit.exe /export /areas USER_RIGHTS /cfg $export | Out-Null
            if($LASTEXITCODE -ne 0){throw "Export LSA impossible : $LASTEXITCODE"}
            $lines=@(Get-Content -LiteralPath $export -Encoding Unicode)
            $currentMap=@{}
            $inRights=$false
            foreach($right in $expected){
                $entries=@(Get-UserRightEntries $lines $right)
                $currentMap[$right]=$entries
                if(($grpSid -and ($grpSid -in $entries)) -or
                    @($entries | Where-Object {$_ -eq $groupName -or $_ -like "*\$groupName"}).Count -gt 0){
                    $inRights=$true
                }
            }
            if($grp -or $inRights){
                $targetRights=@{}
                foreach($right in $expected){
                    $restored=@($currentMap[$right] | Where-Object {
                        (-not $grpSid -or $_ -ne $grpSid) -and $_ -ne $groupName -and $_ -notlike "*\$groupName"
                    })
                    # Sans journal d'origine, ne jamais inventer une attribution à Users (ni à Administrators).
                    $targetRights[$right]=$restored
                }
                Restore-ShutdownRightsInf $tmpRights $targetRights
                if($grp){Remove-LocalGroup -Name $groupName -ErrorAction Stop}
                Emit 'service' 'warning' 'Journal LSA absent : droits d''arrêt restaurés de façon conservatrice et groupe dédié supprimé' 20
            }
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
    # Restauration v2 des raccourcis AVANT toute désinstallation tierce (RDP Wrapper, Apollo, HidHide) : si elle échoue,
    # elle lève et aucun composant tiers n'est retiré. Le service est déjà arrêté ; le MSI HidHide nettoie ensuite ses propres raccourcis.
    $shortcuts=Join-Path $data 'shutdown-rights\shortcuts.json'
    $shortcutBackupDir=Join-Path $data 'shutdown-rights\shortcuts-backup'
    $commonDesktop=[Environment]::GetFolderPath('CommonDesktopDirectory'); if([string]::IsNullOrWhiteSpace($commonDesktop)){$commonDesktop=Join-Path $env:PUBLIC 'Desktop'}
    $commonPrograms=[Environment]::GetFolderPath('CommonPrograms'); if([string]::IsNullOrWhiteSpace($commonPrograms)){$commonPrograms=Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs'}
    # CommonStartMenu (racine) : l'original réel de HidHide Configuration Client y est placé, en plus de CommonPrograms (imbriqué) et du bureau public.
    $commonStartMenu=[Environment]::GetFolderPath('CommonStartMenu'); if([string]::IsNullOrWhiteSpace($commonStartMenu)){$commonStartMenu=Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu'}
    $shortcutRoots=@(@($commonDesktop,$commonStartMenu,$commonPrograms) | Where-Object { $_ } | ForEach-Object { [IO.Path]::GetFullPath($_).TrimEnd('\') } | Select-Object -Unique)
    # Noms autorisés : identiques à SeatShortcutPolicy (y compris la variante mojibake historique).
    $allowedShortcutNames=@('Kinseat Hôte.lnk',('Kinseat H'+[char]0x00C3+[char]0x00B4+'te.lnk'),'HidHide Configurator.lnk','HidHide Configuration Client.lnk')
    function Test-PathUnder($root,$full) {
        $prefix=$root.TrimEnd('\')+'\'
        return ($full.Length -gt $prefix.Length -and $full.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase))
    }
    function Test-ItemExists($p) { return [bool](Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue) }
    function Test-IsReparse($p) {
        $item=Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue
        if(-not $item){return $false}
        return [bool]($item.Attributes -band [IO.FileAttributes]::ReparsePoint)
    }
    # $true si la racine ou un segment existant du chemin est un lien/point d'analyse (aucun lien n'est suivi).
    function Test-ReparseChain($root,$full) {
        $root=$root.TrimEnd('\')
        if(Test-IsReparse $root){return $true}
        $current=$root
        foreach($segment in $full.Substring($root.Length).Split('\',[StringSplitOptions]::RemoveEmptyEntries)){
            $current=Join-Path $current $segment
            if(Test-IsReparse $current){return $true}
        }
        return $false
    }
    function Resolve-ShortcutOriginal($path) {
        if([string]::IsNullOrWhiteSpace($path) -or -not [IO.Path]::IsPathRooted($path)){return $null}
        try{$full=[IO.Path]::GetFullPath($path)}catch{return $null}
        if([IO.Path]::GetFileName($full) -notin $allowedShortcutNames){return $null}
        # Racines imbriquées (StartMenu contient Programs) : toutes les racines englobantes doivent être exemptes de lien, comme le service.
        $matched=$false
        foreach($root in $shortcutRoots){
            if(Test-PathUnder $root $full){
                $matched=$true
                if(Test-ReparseChain $root $full){return $null}
            }
        }
        if($matched){return $full}
        return $null
    }
    function Resolve-ShortcutBackup($path,$originalPath) {
        if([string]::IsNullOrWhiteSpace($path) -or -not [IO.Path]::IsPathRooted($path)){return $null}
        try{$full=[IO.Path]::GetFullPath($path)}catch{return $null}
        $dir=[IO.Path]::GetFullPath($shortcutBackupDir).TrimEnd('\')
        if(-not (Test-PathUnder $dir $full)){return $null}
        if(-not [string]::Equals([IO.Path]::GetDirectoryName($full).TrimEnd('\'),$dir,[StringComparison]::OrdinalIgnoreCase)){return $null}
        $leaf=[IO.Path]::GetFileName($full)
        if($leaf -notmatch '^[0-9A-F]{16}_' -or -not [string]::Equals($leaf.Substring(17),[IO.Path]::GetFileName($originalPath),[StringComparison]::Ordinal)){return $null}
        # Nom déterministe de SeatShortcutPolicy.BuildBackupPath : SHA-256 UTF-8 du chemin d'origine canonique en majuscules, 16 premiers hex.
        try{
            $canonical=[IO.Path]::GetFullPath($originalPath).ToUpperInvariant()
            $sha=[Security.Cryptography.SHA256]::Create()
            try{$hashBytes=$sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($canonical))}finally{$sha.Dispose()}
            $expectedHash=([BitConverter]::ToString($hashBytes) -replace '-','').Substring(0,16)
        }catch{return $null}
        if(-not [string]::Equals($leaf.Substring(0,16),$expectedHash,[StringComparison]::Ordinal)){return $null}
        if(Test-ReparseChain $dir $full){return $null}
        return $full
    }
    function Get-NormalizedSddl($sddl) {
        return ([Security.AccessControl.RawSecurityDescriptor]::new($sddl)).GetSddlForm([Security.AccessControl.AccessControlSections]::Access)
    }
    function Restore-ShortcutAcl($path,$sddl) {
        $acl=Get-Acl -LiteralPath $path
        if(-not [string]::IsNullOrWhiteSpace($sddl)){
            $acl.SetSecurityDescriptorSddlForm($sddl,[Security.AccessControl.AccessControlSections]::Access)
            Set-Acl -LiteralPath $path -AclObject $acl
            $actual=(Get-Acl -LiteralPath $path).GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)
            if((Get-NormalizedSddl $actual) -ne (Get-NormalizedSddl $sddl)){throw "ACL restaurée différente de l'ACL d'origine : $path"}
        } else {
            # Repli de SeatShortcutPolicy : héritage réactivé, règles explicites retirées.
            $acl.SetAccessRuleProtection($false,$false)
            foreach($rule in @($acl.GetAccessRules($true,$false,[Security.Principal.SecurityIdentifier]))){[void]$acl.RemoveAccessRuleSpecific($rule)}
            Set-Acl -LiteralPath $path -AclObject $acl
        }
    }
    function Write-ShortcutManifest($entries) {
        $manifest=[ordered]@{Version=2;Entries=@($entries)}
        $tmp=$shortcuts+'.tmp'
        [IO.File]::WriteAllText($tmp,(ConvertTo-Json -InputObject $manifest -Depth 6),(New-Object Text.UTF8Encoding($false)))
        Move-Item -LiteralPath $tmp -Destination $shortcuts -Force
    }
    if(Test-Path -LiteralPath $shortcuts){
        $rawShortcuts=(Get-Content -LiteralPath $shortcuts -Raw -Encoding UTF8).Trim()
        if($rawShortcuts.StartsWith('[')){
            # Manifeste v1 (anciennes ACE Deny Read) : seules ces ACE historiques sont retirées.
            foreach($entry in @($rawShortcuts | ConvertFrom-Json)){
                if($entry.Sid -notmatch '^S-1-5-21-(\d+-){3}\d+$' -or
                    [IO.Path]::GetFileName($entry.Path) -notin $allowedShortcutNames){
                    throw 'Inventaire de raccourcis invalide : désinstallation refusée'
                }
                $v1Path=Resolve-ShortcutOriginal $entry.Path
                if(-not $v1Path){Emit 'shortcuts' 'warning' "Entrée v1 ignorée (hors périmètre ou lien) : $($entry.Path)" 62; continue}
                if(Test-Path -LiteralPath $v1Path){
                    $acl=Get-Acl -LiteralPath $v1Path
                    $rule=[Security.AccessControl.FileSystemAccessRule]::new(
                        [Security.Principal.SecurityIdentifier]::new($entry.Sid),
                        [Security.AccessControl.FileSystemRights]::Read,
                        [Security.AccessControl.AccessControlType]::Deny)
                    [void]$acl.RemoveAccessRuleSpecific($rule)
                    Set-Acl -LiteralPath $v1Path -AclObject $acl
                }
            }
            Remove-Item -LiteralPath $shortcuts -Force
        } else {
            # Manifeste v2 : relocalisation des .lnk vers shortcuts-backup ; chaque fichier doit être rendu à son OriginalPath.
            $manifest=$rawShortcuts | ConvertFrom-Json
            if($manifest.Version -ne 2 -or $null -eq $manifest.Entries){throw 'Manifeste de raccourcis invalide : désinstallation refusée, sauvegarde conservée'}
            $remaining=New-Object Collections.Generic.List[object]
            $failures=New-Object Collections.Generic.List[string]
            foreach($entry in @($manifest.Entries)){
                $orig=Resolve-ShortcutOriginal $entry.OriginalPath
                $bak=$null
                if($orig){$bak=Resolve-ShortcutBackup $entry.BackupPath $orig}
                if(-not $orig -or -not $bak){
                    $failures.Add("Entrée refusée (chemin hors périmètre ou lien) : $($entry.OriginalPath)")
                    $remaining.Add($entry); continue
                }
                if($entry.State -notin @('PendingMove','Relocated','PendingRestore','RestoreConflict')){
                    $failures.Add("État de raccourci inconnu '$($entry.State)' : $orig"); $remaining.Add($entry); continue
                }
                try{
                    $origExists=Test-ItemExists $orig
                    $bakExists=Test-ItemExists $bak
                    if($entry.State -eq 'PendingMove' -and -not $bakExists){
                        # Déplacement jamais effectué (ou rien à restaurer) : aucune sauvegarde à perdre.
                        continue
                    }
                    if(-not $bakExists){
                        if($origExists){Restore-ShortcutAcl $orig $entry.OriginalSddl}
                        else{Emit 'shortcuts' 'warning' "Sauvegarde absente, raccourci non récupérable : $orig" 62}
                        continue
                    }
                    if($origExists){
                        $failures.Add("Conflit : $orig existe déjà ; fichier conservé, sauvegarde maintenue : $bak")
                        $remaining.Add($entry); continue
                    }
                    $parent=[IO.Path]::GetDirectoryName($orig)
                    if(-not (Test-Path -LiteralPath $parent -PathType Container)){[void](New-Item -ItemType Directory -Path $parent)}
                    if(Test-ReparseChain ($shortcutRoots | Where-Object { Test-PathUnder $_ $orig } | Select-Object -First 1) $orig){throw "Répertoire parent redirigé : $parent"}
                    # File.Move n'écrase jamais : échec si le raccourci a été recréé entre-temps.
                    [IO.File]::Move($bak,$orig)
                    try{ Restore-ShortcutAcl $orig $entry.OriginalSddl }
                    catch{
                        $aclError=$_.Exception.Message
                        try{ [IO.File]::Move($orig,$bak) }catch{ $aclError+=" ; retour arrière impossible, fichier laissé à $orig" }
                        throw $aclError
                    }
                    if(-not (Test-ItemExists $orig) -or (Test-ItemExists $bak)){throw "Restauration non vérifiée : $orig"}
                    Emit 'shortcuts' 'ok' "Raccourci restauré : $orig" 62
                } catch {
                    $failures.Add("Restauration impossible pour $orig : $($_.Exception.Message)")
                    $remaining.Add($entry)
                }
            }
            if($failures.Count -gt 0){
                Write-ShortcutManifest $remaining
                throw ('Raccourcis non restaurés, manifeste et sauvegarde conservés : ' + ($failures -join ' | '))
            }
            $leftovers=@()
            if(Test-Path -LiteralPath $shortcutBackupDir){
                if(Test-IsReparse $shortcutBackupDir){throw 'Répertoire de sauvegarde redirigé : désinstallation refusée'}
                $leftovers=@(Get-ChildItem -LiteralPath $shortcutBackupDir -Force)
            }
            if($leftovers.Count -gt 0){throw "Fichiers inattendus dans shortcuts-backup, conservés : $($leftovers.Count)"}
            # Restauration complète et vérifiée : le manifeste puis le répertoire de sauvegarde (vide) peuvent être supprimés.
            Remove-Item -LiteralPath $shortcuts -Force
            Remove-Item -LiteralPath ($shortcuts+'.tmp') -Force -ErrorAction SilentlyContinue
            if(Test-Path -LiteralPath $shortcutBackupDir){Remove-Item -LiteralPath $shortcutBackupDir -Force}
        }
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
    Remove-Item 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Kinseat' -Recurse -Force -ErrorAction SilentlyContinue
    # Retrait des seuls raccourcis publics prouvés créés par cette installation, APRÈS la restauration du manifeste v2
    # (un échec de restauration lève avant ce point). Preuve : nom Kinseat, racine publique, aucun lien, cible dans le dossier
    # d'installation réel et empreinte SHA-256 identique à l'inventaire. Inventaire absent (ancienne version) : rien n'est supprimé.
    $kinseatShortcutNames=@($allowedShortcutNames[0],$allowedShortcutNames[1])
    $ownedInventory=@()
    if($state.PSObject.Properties['shortcuts'] -and $state.shortcuts){$ownedInventory=@($state.shortcuts)}
    if($ownedInventory.Count -eq 0){Emit 'shortcuts' 'ok' 'Aucun inventaire de raccourcis Kinseat : raccourcis publics conservés' 63}
    else{
        $installReal=$null
        if((Test-Path -LiteralPath $install -PathType Container) -and -not (Test-IsReparse $install)){$installReal=(Resolve-Path -LiteralPath $install).Path.TrimEnd('\')}
        $keptInventory=New-Object Collections.Generic.List[object]
        $wsh=$null
        foreach($owned in $ownedInventory){
            $keep=$true
            try{
                $p=Resolve-ShortcutOriginal ([string]$owned.path)
                if(-not $p -or [IO.Path]::GetFileName($p) -notin $kinseatShortcutNames){Emit 'shortcuts' 'warning' "Inventaire ignoré (hors périmètre) : $($owned.path)" 63}
                elseif(-not $installReal){Emit 'shortcuts' 'warning' "Dossier d'installation introuvable ou redirigé, raccourci conservé : $p" 63}
                elseif(-not (Test-ItemExists $p)){$keep=$false}
                elseif(Test-IsReparse $p){Emit 'shortcuts' 'warning' "Raccourci redirigé conservé : $p" 63}
                else{
                    if(-not $wsh){$wsh=New-Object -ComObject WScript.Shell}
                    $target=[string]$wsh.CreateShortcut($p).TargetPath
                    $hash=(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash
                    $targetOk=$target -and [IO.Path]::IsPathRooted($target) -and (Test-PathUnder $installReal ([IO.Path]::GetFullPath($target))) -and [string]::Equals([IO.Path]::GetFileName($target),'kinseat.exe',[StringComparison]::OrdinalIgnoreCase)
                    if($targetOk -and [string]::Equals($hash,[string]$owned.sha256,[StringComparison]::OrdinalIgnoreCase)){
                        Remove-Item -LiteralPath $p -Force
                        Emit 'shortcuts' 'ok' "Raccourci Kinseat créé par l'installation supprimé : $p" 63
                        $keep=$false
                    } else {Emit 'shortcuts' 'warning' "Raccourci modifié ou cible non Kinseat, conservé : $p" 63}
                }
            }catch{Emit 'shortcuts' 'warning' "Raccourci conservé (vérification impossible) : $($owned.path) : $($_.Exception.Message)" 63}
            if($keep){$keptInventory.Add($owned)}
        }
        $state | Add-Member -NotePropertyName shortcuts -NotePropertyValue @($keptInventory.ToArray()) -Force
    }
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
    else { $state.status='uninstalled'; [IO.File]::WriteAllText($stateFile,(ConvertTo-Json -InputObject $state -Depth 8),(New-Object Text.UTF8Encoding($false))) }
    Emit 'complete' 'ok' 'Désinstallation terminée (voir avertissements)' 100
} catch {Emit 'failure' 'error' $_.Exception.Message 0; exit 1}
