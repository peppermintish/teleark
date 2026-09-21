# Check the Git index so the result does not depend on host checkout settings.
param(
    [string]$Repository = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$repoPath = [System.IO.Path]::GetFullPath($Repository)
$trackedCount = 0
$textCount = 0
$binaryCount = 0
$crlfPaths = [System.Collections.Generic.List[string]]::new()
$forbiddenPaths = [System.Collections.Generic.List[string]]::new()

Push-Location -LiteralPath $repoPath
try {
    $lines = @(& git -c core.quotepath=false ls-files --eol)
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not list tracked files and their line endings.'
    }

    foreach ($line in $lines) {
        $parts = $line -split "`t", 2
        if ($parts.Count -ne 2 -or $parts[0] -notmatch '^i/(?<index>\S+)\s+w/\S+\s+attr/.*$') {
            throw "Unexpected Git line-ending record: $line"
        }

        $trackedCount++
        $indexEnding = $Matches.index
        $path = $parts[1]
        if ($indexEnding -eq '-text') {
            $binaryCount++
        } else {
            $textCount++
            if ($indexEnding -in @('crlf', 'mixed')) {
                $crlfPaths.Add($path)
            }
        }
        if ($path -match '(?i)\.(cmd|bat)$') {
            $forbiddenPaths.Add($path)
        }
    }
} finally {
    Pop-Location
}

$failed = $crlfPaths.Count -gt 0 -or $forbiddenPaths.Count -gt 0
$result = if ($failed) { 'FAIL' } else { 'PASS' }
$report = [System.Collections.Generic.List[string]]::new()
$report.Add('### Line-ending policy')
$report.Add('')
$report.Add('| Result | Tracked files | Plain-text files | Binary files | CRLF files | Forbidden scripts |')
$report.Add('| :--- | ---: | ---: | ---: | ---: | ---: |')
$report.Add("| $result | $trackedCount | $textCount | $binaryCount | $($crlfPaths.Count) | $($forbiddenPaths.Count) |")
$report.Add('')
$report.Add('Checked Git-index content, which is independent of the runner platform. All tracked plain text must use LF. `.cmd` and `.bat` files are forbidden because they depend on Windows line endings.')
if ($crlfPaths.Count -gt 0) {
    $report.Add('')
    $report.Add('**Files with CRLF:**')
    foreach ($path in $crlfPaths) {
        $report.Add("- ``$($path.Replace('`', '``'))``")
    }
}
if ($forbiddenPaths.Count -gt 0) {
    $report.Add('')
    $report.Add('**Forbidden scripts:**')
    foreach ($path in $forbiddenPaths) {
        $report.Add("- ``$($path.Replace('`', '``'))``")
    }
}

if ($env:GITHUB_STEP_SUMMARY) {
    Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value ($report -join "`n") -Encoding utf8
}
Write-Output "Line endings: $result; tracked=$trackedCount text=$textCount binary=$binaryCount CRLF=$($crlfPaths.Count) forbidden=$($forbiddenPaths.Count)"
foreach ($path in $crlfPaths) {
    Write-Output "::error file=$path::Tracked plain-text file contains CRLF. Save it with LF line endings."
}
foreach ($path in $forbiddenPaths) {
    Write-Output "::error file=$path::Batch scripts (.cmd and .bat) are forbidden by the LF-only repository policy."
}
if ($failed) { exit 1 }
