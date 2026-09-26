# Synthetic checksum manifests; no installers, credentials or existing artifacts are read.
$ErrorActionPreference = 'Stop'
$checker = Join-Path $PSScriptRoot 'check-release-checksums.ps1'
$stage = Join-Path ([IO.Path]::GetTempPath()) "teleark-checksums-$([guid]::NewGuid().ToString('N'))"
$names = @('teleark-test.exe', 'teleark-test.zip', 'TeleArk-Setup-test.exe')
$checks = 0

function Assert-Manifest([string[]]$Lines, [bool]$ShouldPass) {
    [IO.File]::WriteAllLines((Join-Path $stage 'SHA256SUMS'), $Lines)
    $failure = $null
    try { & $checker -Directory $stage -ExpectedFiles $names | Out-Null } catch { $failure = $_ }
    if (($null -eq $failure) -ne $ShouldPass) {
        throw "Unexpected checksum result for synthetic case $script:checks."
    }
    $script:checks++
}

try {
    New-Item -ItemType Directory -Path $stage | Out-Null
    $valid = foreach ($name in $names) {
        $path = Join-Path $stage $name
        [IO.File]::WriteAllText($path, "synthetic $name")
        "$((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant())  $name"
    }
    Assert-Manifest $valid $true
    # Unix verifiers invoke pwsh -File, which must collect every filename argument.
    $output = & (Get-Process -Id $PID).Path -NoProfile -File $checker -Directory $stage @names 2>&1
    if ($LASTEXITCODE -ne 0) { throw "External checksum invocation failed: $output" }
    $checks++
    Assert-Manifest @($valid[2], $valid[0], $valid[1]) $true
    Assert-Manifest @($valid[0], $valid[0], $valid[0]) $false
    Assert-Manifest @($valid[0], $valid[1]) $false
    Assert-Manifest @($valid + $valid[0]) $false
    Assert-Manifest @($valid[0], $valid[1], ('0' * 64) + "  $($names[2])") $false
    Assert-Manifest @($valid[0], $valid[1], ('a' * 64) + '  ../outside.exe') $false
    Assert-Manifest @($valid[0], $valid[1], ('a' * 64) + '  unknown.exe') $false
    Assert-Manifest @($valid[0], $valid[1], 'malformed') $false
    [IO.File]::WriteAllText((Join-Path $stage $names[2]), 'changed payload')
    Assert-Manifest $valid $false
    Remove-Item -LiteralPath (Join-Path $stage $names[2])
    Assert-Manifest $valid $false
    [IO.File]::WriteAllBytes((Join-Path $stage $names[2]), [byte[]]@())
    $emptyHash = (Get-FileHash -LiteralPath (Join-Path $stage $names[2]) -Algorithm SHA256).Hash
    Assert-Manifest @($valid[0], $valid[1], "$emptyHash  $($names[2])") $false
} finally {
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
Write-Output "Release checksum validation passed $checks synthetic cases."
