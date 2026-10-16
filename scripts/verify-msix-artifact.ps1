[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Directory,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9][A-Za-z0-9._-]*\.msix$')][string]$ExpectedFileName
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$directoryPath = [System.IO.Path]::GetFullPath($Directory)
if (-not (Test-Path -LiteralPath $directoryPath -PathType Container)) {
    throw 'Downloaded Windows package directory does not exist.'
}

$expectedEntries = @('SHA256SUMS', $ExpectedFileName) | Sort-Object -CaseSensitive
$actualEntries = @(Get-ChildItem -LiteralPath $directoryPath -Force | ForEach-Object { $_.Name } | Sort-Object -CaseSensitive)
if ($actualEntries.Count -ne $expectedEntries.Count -or
    [string]::Join("`n", $actualEntries) -cne [string]::Join("`n", $expectedEntries)) {
    throw 'Downloaded Windows package files do not match the required MSIX and checksum manifest.'
}

$checksumPath = Join-Path $directoryPath 'SHA256SUMS'
$checksumLines = @([System.IO.File]::ReadAllLines($checksumPath))
if ($checksumLines.Count -ne 1) {
    throw 'Windows package checksum manifest must contain exactly one entry.'
}

$match = [System.Text.RegularExpressions.Regex]::Match(
    $checksumLines[0],
    '^(?<hash>[A-Fa-f0-9]{64})  (?<file>[^/\\]+)$'
)
if (-not $match.Success -or $match.Groups['file'].Value -cne $ExpectedFileName) {
    throw 'Windows package checksum manifest does not name the expected MSIX.'
}

$actualHash = (Get-FileHash -LiteralPath (Join-Path $directoryPath $ExpectedFileName) -Algorithm SHA256).Hash
if ($actualHash -cne $match.Groups['hash'].Value.ToUpperInvariant()) {
    throw 'Downloaded Windows package checksum verification failed.'
}

Write-Output 'Verified the downloaded Windows package and its SHA-256 checksum.'
