# Validate exactly the expected flat artifact set before invoking native installers.
param(
    [Parameter(Mandatory)][string]$Directory,
    [Parameter(Mandatory, ValueFromRemainingArguments)][string[]]$ExpectedFiles
)
$ErrorActionPreference = 'Stop'
$Directory = (Resolve-Path -LiteralPath $Directory).Path
$remaining = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($name in $ExpectedFiles) {
    if ($name -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*\z' -or -not $remaining.Add($name)) {
        throw 'Expected release artifacts must have unique, plain filenames.'
    }
}
$manifest = Join-Path $Directory 'SHA256SUMS'
foreach ($line in [IO.File]::ReadAllLines($manifest)) {
    if ($line -cnotmatch '^(?<hash>[0-9a-fA-F]{64})  (?<name>[A-Za-z0-9][A-Za-z0-9._-]*)\z') {
        throw 'Malformed release checksum entry; expected SHA256, two spaces and a plain filename.'
    }
    $name = $Matches.name
    $expectedHash = $Matches.hash
    if (-not $remaining.Remove($name)) {
        throw "Unexpected or duplicate release checksum entry: $name"
    }
    $path = Join-Path $Directory $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or
        (Get-Item -LiteralPath $path).Length -eq 0 -or
        (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ine $expectedHash) {
        throw "Release checksum verification failed for $name"
    }
}
if ($remaining.Count -ne 0) {
    throw "Release checksum manifest is missing: $(@($remaining | Sort-Object) -join ', ')"
}
Write-Output "Verified SHA256 checksums for all $($ExpectedFiles.Count) expected release artifacts."
