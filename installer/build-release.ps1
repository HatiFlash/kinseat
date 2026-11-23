#Requires -Version 5.1
<# Assemble un paquet depuis des binaires déjà construits ; aucune installation. #>
param([Parameter(Mandatory=$true)][string]$ServicePublishDir,
      [Parameter(Mandatory=$true)][string]$HostExe,
      [Parameter(Mandatory=$true)][string]$AgentExe,
      [Parameter(Mandatory=$true)][string]$OutDir,
      # Dossier facultatif contenant des archives tierces qualifiées (noms du manifeste source).
      [string]$ThirdPartyDir='')
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$dist=Join-Path $OutDir 'host-payload-0.3.18'
$payload=Join-Path $dist 'payload'
# Charge utile embarquée ensuite dans kinseat-setup (KINSEAT_HOST_PAYLOAD_ZIP) : on ne livre qu'un .exe.
$zip=Join-Path $OutDir 'host-payload-0.3.18.zip'
if ((Test-Path -LiteralPath $dist) -or (Test-Path -LiteralPath $zip)) { throw 'Sortie existante : suppression manuelle requise' }
foreach($file in @($HostExe,$AgentExe)) { if(-not (Test-Path -LiteralPath $file -PathType Leaf)){throw "Binaire absent : $file"} }
if(-not (Test-Path -LiteralPath (Join-Path $ServicePublishDir 'Kinseat.Service.exe')) -or -not (Test-Path -LiteralPath (Join-Path $ServicePublishDir 'wwwroot/index.html'))) { throw 'Publication service incomplète' }
foreach($name in @('Kinseat.SteamLauncher.exe','Kinseat.SteamHook.dll','Kinseat.SteamHook32.dll','Kinseat.GameFiles.dll','Kinseat.GameFiles32.dll')) {
    if(-not (Test-Path -LiteralPath (Join-Path $ServicePublishDir "SteamIsolation/$name") -PathType Leaf)) { throw "Publication isolation incomplète : $name" }
}
New-Item -ItemType Directory -Path (Join-Path $payload 'service') -Force | Out-Null
try {
    Get-ChildItem -LiteralPath $ServicePublishDir -Force | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $payload 'service') -Recurse -Force }
    Copy-Item -LiteralPath $HostExe -Destination (Join-Path $payload 'kinseat.exe')
    Copy-Item -LiteralPath $AgentExe -Destination (Join-Path $payload 'kinseat-agent.exe')
    foreach($name in @('Install-Kinseat.ps1','Uninstall-Kinseat.ps1','MultiSession-Kinseat.ps1','rdpwrap-offsets.ini')) { Copy-Item -LiteralPath (Join-Path $PSScriptRoot "payload/$name") -Destination (Join-Path $payload $name) }
    # Licences : Kinseat (MIT) et composants tiers GPL redistribués (notice + texte de licence).
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'licenses') -Destination (Join-Path $payload 'licenses') -Recurse -Force
    $source=Get-Content -LiteralPath (Join-Path $PSScriptRoot 'manifest.json') -Raw | ConvertFrom-Json
    $optional=@{}
    if($ThirdPartyDir) {
        foreach($entry in $source.artifacts | Where-Object { -not $_.required }) {
            $candidate=Join-Path $ThirdPartyDir $entry.file
            if(Test-Path -LiteralPath $candidate -PathType Leaf) { Copy-Item -LiteralPath $candidate -Destination (Join-Path $payload $entry.file); $optional[$entry.file]=$entry; Write-Host "Composant tiers inclus : $($entry.file)" }
        }
    }
    $items=New-Object System.Collections.ArrayList
    foreach($file in @(Get-ChildItem -LiteralPath $payload -File -Recurse)) {
        $relative=$file.FullName.Substring($payload.Length+1).Replace('\','/')
        $id=$relative; $version='0.3.18'; $required=$true
        foreach($entry in $source.artifacts){if($entry.file -eq $relative){$id=$entry.id;break}}
        if($optional.ContainsKey($relative)){ $version=$optional[$relative].version; $required=$false }
        [void]$items.Add([ordered]@{id=$id;version=$version;file=$relative;sha256=(Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant();required=$required})
    }
    foreach($entry in $source.artifacts | Where-Object { -not $_.required }) {
        if(-not @($items | Where-Object { $_.file -eq $entry.file }).Count) { [void]$items.Add($entry) }
    }
    $manifest=[ordered]@{version='0.3.18';artifacts=@($items.ToArray())}
    $manifestPath=Join-Path $payload 'manifest.json'
    [IO.File]::WriteAllText($manifestPath,(ConvertTo-Json -InputObject $manifest -Depth 8),(New-Object Text.UTF8Encoding($false)))
    $readback=Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    foreach($item in $readback.artifacts | Where-Object { $_.required }) {
        $file=Join-Path $payload $item.file
        if(-not (Test-Path -LiteralPath $file -PathType Leaf) -or (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash -ne $item.sha256){throw "Manifeste incorrect : $($item.file)"}
    }
    foreach($required in @('Install-Kinseat.ps1','Uninstall-Kinseat.ps1','MultiSession-Kinseat.ps1','rdpwrap-offsets.ini','service/Kinseat.Service.exe','service/SteamIsolation/Kinseat.GameFiles.dll','service/SteamIsolation/Kinseat.GameFiles32.dll','kinseat.exe','kinseat-agent.exe')) {if(-not @($readback.artifacts | Where-Object { $_.file -eq $required -and $_.required }).Count){throw "Manquant : $required"}}
    # .NET ZipFile écrit des séparateurs « / » (Compress-Archive 5.1 écrit des « \ »).
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::CreateFromDirectory($payload, $zip, [IO.Compression.CompressionLevel]::Optimal, $false)
    Write-Host "Charge utile : $zip (dossier conservé pour l'analyse antivirus : $payload)"
} catch { if(Test-Path -LiteralPath $dist){Remove-Item -LiteralPath $dist -Recurse -Force}; throw }
