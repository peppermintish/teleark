# Synthetic repository checks for the cross-platform line-ending policy.
$ErrorActionPreference = 'Stop'
$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$repository = [System.IO.Path]::GetFullPath(
    (Join-Path $tempRoot "teleark-line-endings-$([guid]::NewGuid().ToString('N'))")
)
$checker = Join-Path $PSScriptRoot 'check-line-endings.ps1'

function Invoke-Check {
    $summary = Join-Path $repository 'summary.md'
    if (Test-Path -LiteralPath $summary) {
        Remove-Item -LiteralPath $summary -Force
    }
    $env:GITHUB_STEP_SUMMARY = $summary
    try {
        $output = & pwsh -NoProfile -File $checker -Repository $repository 2>&1
        return @{ Code = $LASTEXITCODE; Output = ($output -join "`n"); Summary = (Get-Content -LiteralPath $summary -Raw) }
    } finally {
        Remove-Item Env:GITHUB_STEP_SUMMARY -ErrorAction SilentlyContinue
    }
}

try {
    New-Item -ItemType Directory -Path $repository | Out-Null
    & git -C $repository init --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Could not create the test repository.' }

    [System.IO.File]::WriteAllText((Join-Path $repository 'good.txt'), "one`ntwo`n")
    [System.IO.File]::WriteAllBytes((Join-Path $repository 'image.bin'), [byte[]](137, 80, 78, 71, 13, 10, 26, 10, 0))
    & git -C $repository -c core.autocrlf=false add good.txt image.bin
    if ($LASTEXITCODE -ne 0) { throw 'Could not stage the clean fixtures.' }
    $clean = Invoke-Check
    if ($clean.Code -ne 0 -or $clean.Output -notmatch 'CRLF=0 forbidden=0' -or $clean.Summary -notmatch 'PASS') {
        throw 'LF text and binary CRLF must pass.'
    }

    [System.IO.File]::WriteAllText((Join-Path $repository 'bad.txt'), "one`r`ntwo`r`n")
    & git -C $repository -c core.autocrlf=false add bad.txt
    if ($LASTEXITCODE -ne 0) { throw 'Could not stage the CRLF fixture.' }
    $crlf = Invoke-Check
    if ($crlf.Code -ne 1 -or $crlf.Output -notmatch 'CRLF=1 forbidden=0' -or $crlf.Summary -notmatch 'bad.txt') {
        throw 'A tracked CRLF text file must fail and appear in the report.'
    }

    [System.IO.File]::WriteAllText((Join-Path $repository 'bad.txt'), "one`ntwo`n")
    [System.IO.File]::WriteAllText((Join-Path $repository 'legacy.cmd'), "@echo off`n")
    [System.IO.File]::WriteAllText((Join-Path $repository 'legacy.bat'), "@echo off`n")
    & git -C $repository -c core.autocrlf=false add bad.txt legacy.cmd legacy.bat
    if ($LASTEXITCODE -ne 0) { throw 'Could not stage the batch-script fixtures.' }
    $batch = Invoke-Check
    if ($batch.Code -ne 1 -or $batch.Output -notmatch 'CRLF=0 forbidden=2' -or $batch.Summary -notmatch 'legacy.cmd' -or $batch.Summary -notmatch 'legacy.bat') {
        throw 'LF-only .cmd and .bat files must fail and appear in the report.'
    }

    Write-Output 'Line-ending policy tests passed.'
} finally {
    $prefix = Join-Path $tempRoot 'teleark-line-endings-'
    if (-not $repository.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a test directory outside the designated temp root.'
    }
    if (Test-Path -LiteralPath $repository) {
        Remove-Item -LiteralPath $repository -Recurse -Force
    }
}

# GitHub's pwsh wrapper exits with the last native command's status. The
# expected rejection above leaves that status at 1 even though every test passed.
$global:LASTEXITCODE = 0
