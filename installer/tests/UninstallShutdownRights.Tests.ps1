#Requires -Version 5.1
# Test autonome (sans Pester, sans Windows) de Get-ShutdownRightTarget (Uninstall-Kinseat.ps1).
# Code de sortie : 0 si tout passe, 1 sinon. Compatible Windows PowerShell 5.1 et pwsh.
$ErrorActionPreference = 'Stop'
$scriptPath = Join-Path $PSScriptRoot '../payload/Uninstall-Kinseat.ps1'
$tokens = $null; $parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path $scriptPath).Path, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors -and $parseErrors.Count -gt 0) { Write-Host "ECHEC : erreur de syntaxe dans Uninstall-Kinseat.ps1 : $($parseErrors[0].Message)"; exit 1 }
foreach ($fname in @('Get-ShutdownRightTarget', 'ConvertTo-RightSidSet')) {
    $def = $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $fname }, $true) | Select-Object -First 1
    if (-not $def) { Write-Host "ECHEC : fonction $fname introuvable"; exit 1 }
    . ([scriptblock]::Create($def.Extent.Text))
}

$script:failures = 0
function Assert-Set($name, $actual, $expected) {
    $a = (@($actual) | Sort-Object) -join ','
    $e = (@($expected) | Sort-Object) -join ','
    if ($a -ceq $e) { Write-Host "OK    $name" }
    else { Write-Host "ECHEC $name : obtenu [$a] attendu [$e]"; $script:failures++ }
}

$group = 'Kinseat-ArretAutorise'
$gsid = 'S-1-5-21-1111111111-2222222222-3333333333-1001'
$admins = 'S-1-5-32-544'; $users = 'S-1-5-32-545'; $backup = 'S-1-5-32-551'
$shutdownChange = [pscustomobject]@{ Right = 'SeShutdownPrivilege'; Add = @($gsid); Remove = @($users) }
$remoteChange = [pscustomobject]@{ Right = 'SeRemoteShutdownPrivilege'; Add = @(); Remove = @() }

# 1) Scénario VM : groupe exporté par NOM, journal Remove=Users.
$t = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' @($admins, $backup, $group) $shutdownChange $group $gsid)
Assert-Set 'VM arrêt local (groupe par nom, journal)' $t @($admins, $users, $backup)
$r = @(Get-ShutdownRightTarget 'SeRemoteShutdownPrivilege' @($admins, $group) $remoteChange $group $gsid)
Assert-Set 'VM arrêt distant (groupe par nom, journal)' $r @($admins)

# 2) Groupe exporté par SID.
$t = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' @($admins, $backup, $gsid) $shutdownChange $group $gsid)
Assert-Set 'groupe par SID, journal' $t @($admins, $users, $backup)

# 3) Forme MACHINE\nom.
$t = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' @($admins, $backup, "WIN-TEST\$group") $shutdownChange $group $gsid)
Assert-Set 'groupe MACHINE\nom, journal' $t @($admins, $users, $backup)

# 4) Branche sans journal.
$t = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' @($group, $admins) $null $group $gsid)
Assert-Set 'sans journal : arrêt local = défaut client' $t @($admins, $users, $backup)
$t = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' @($gsid, "WIN-TEST\$group", $admins, 'S-1-5-32-999') $null $group $gsid)
Assert-Set 'sans journal : conserve les autres entrées, retire SID/MACHINE\nom' $t @($admins, $users, $backup, 'S-1-5-32-999')
$r = @(Get-ShutdownRightTarget 'SeRemoteShutdownPrivilege' @($group, $admins) $null $group $gsid)
Assert-Set 'sans journal : arrêt distant sans Users' $r @($admins)
$r = @(Get-ShutdownRightTarget 'SeRemoteShutdownPrivilege' @($gsid, $group, "WIN-TEST\$group", $admins) $null $group $null)
Assert-Set 'sans journal, SID groupe nul : retire nom et MACHINE\nom, conserve le SID' $r @($gsid, $admins)
$r = @(Get-ShutdownRightTarget 'SeRemoteShutdownPrivilege' @($gsid) $null $group $gsid)
Assert-Set 'sans journal : arrêt distant vide reste vide' $r @()

# 5) Idempotence : un second passage sur la sortie donne le même résultat.
$first = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' @($group, $admins) $null $group $gsid)
$second = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' $first $null $group $gsid)
Assert-Set 'idempotence sans journal' $second $first
$firstJ = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' @($admins, $backup, $group) $shutdownChange $group $gsid)
$secondJ = @(Get-ShutdownRightTarget 'SeShutdownPrivilege' $firstJ $shutdownChange $group $gsid)
Assert-Set 'idempotence avec journal' $secondJ $firstJ

# 6) Normalisation en SID pour la vérification par export (résolveur simulé).
$resolver = { param($n) if ($n -eq 'Administrateurs') { 'S-1-5-32-544' } elseif ($n -eq 'Utilisateurs') { 'S-1-5-32-545' } else { throw 'non résolu' } }
$exp = @(ConvertTo-RightSidSet @($admins, "*$users") $resolver)
$act = @(ConvertTo-RightSidSet @('Administrateurs', 'Utilisateurs') $resolver)
Assert-Set 'SID attendu = nom localisé exporté' $act $exp
Assert-Set 'entrée *S-… normalisée' (ConvertTo-RightSidSet @('*s-1-5-32-544') $resolver) @($admins)
Assert-Set 'nom non résoluble conservé en majuscules' (ConvertTo-RightSidSet @('Inconnu', $admins) $resolver) @('INCONNU', $admins)
$a = (@(ConvertTo-RightSidSet @($admins, $users) $resolver) -join ',')
$b = (@(ConvertTo-RightSidSet @($admins) $resolver) -join ',')
if ($a -ne $b) { Write-Host 'OK    ensembles différents = inégaux' } else { Write-Host 'ECHEC ensembles différents jugés égaux'; $script:failures++ }
$a = (@(ConvertTo-RightSidSet @($users, $admins) $resolver) -join ',')
$b = (@(ConvertTo-RightSidSet @('administrateurs', "*$users") $resolver) -join ',')
if ($a -eq $b) { Write-Host 'OK    ordre/casse sans effet' } else { Write-Host 'ECHEC ordre/casse'; $script:failures++ }

if ($script:failures -gt 0) { Write-Host "$($script:failures) échec(s)"; exit 1 }
Write-Host 'Tous les tests passent'
exit 0
