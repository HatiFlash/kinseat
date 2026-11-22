# Tests Pester 5 : uniquement les fonctions pures du script, sans installation Windows.
$scriptPath = Join-Path $PSScriptRoot '../payload/MultiSession-Kinseat.ps1'
$source = [IO.File]::ReadAllText((Resolve-Path $scriptPath))
$helpers = [regex]::Match($source, '(?s)function Ini-Section\(.*?(?=\r?\nfunction Restart-TermService)').Value
if (-not $helpers) { throw 'Fonctions de sélection introuvables' }
. ([scriptblock]::Create($helpers))
$ini = [IO.File]::ReadAllText((Resolve-Path (Join-Path $PSScriptRoot '../payload/rdpwrap-offsets.ini')))
$version = '10.0.26100.9818'
$hash = '78A3819896BD4A43D696E41A7E7610213E2B4E686D27D5B092869DBCB36910C7'
Describe 'rdpwrap.ini toujours en CRLF' {
    It 'ne produit aucun LF isolé, avec ou sans offsets livrés' {
        $joinedNoOffsets = Join-RdpWrapIni ($ini -replace "`r`n", "`n") $null
        $joinedWithOffsets = Join-RdpWrapIni ($ini -replace "`r`n", "`n") ($ini -replace "`r`n", "`n")
        foreach ($text in @($joinedNoOffsets, $joinedWithOffsets)) {
            ([regex]::Matches($text, '(?<!\r)\n')).Count | Should Be 0
        }
    }
}

Describe 'Offsets livrés' {
    It 'accepte uniquement la version et son empreinte vérifiées' {
        (Select-ValidatedOffsets $ini $version $hash) | Should Match '\[10\.0\.26100\.9818-SLInit\]'
        (Select-ValidatedOffsets $ini '10.0.26100.9819' $hash) | Should BeNullOrEmpty
        (Select-ValidatedOffsets $ini $version ('0' * 64)) | Should BeNullOrEmpty
    }
    It 'refuse une section dont une clé x64 obligatoire manque' {
        $incomplete = $ini -replace '(?m)^bInitialized\.x64=.*\r?\n?', ''
        (Ini-Covers $incomplete $version) | Should Be $false
        (Select-ValidatedOffsets $incomplete $version $hash) | Should BeNullOrEmpty
    }
    It 'refuse une empreinte différente sur la deuxième section' {
        $tampered = $ini -replace '(?m)^; sha256=([0-9A-F]{64})\r?\n\[10\.0\.26100\.9818-SLInit\]', "; sha256=$('0' * 64)`n[10.0.26100.9818-SLInit]"
        (Select-ValidatedOffsets $tampered $version $hash) | Should BeNullOrEmpty
    }
}
