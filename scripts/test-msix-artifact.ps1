# Synthetic download-artifact cases; no real package or credentials are used.
$ErrorActionPreference = 'Stop'
$verifier = Join-Path $PSScriptRoot 'verify-msix-artifact.ps1'
$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testDirectory = [System.IO.Path]::GetFullPath(
    (Join-Path $tempRoot "teleark-msix-artifact-$([guid]::NewGuid().ToString('N'))")
)
$expectedFileName = 'TeleArk-1.2.3-windows-x86_64.msix'
$packagePath = Join-Path $testDirectory $expectedFileName
$checksumPath = Join-Path $testDirectory 'SHA256SUMS'

function Write-Checksum {
    $hash = (Get-FileHash -LiteralPath $packagePath -Algorithm SHA256).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText($checksumPath, "$hash  $expectedFileName`n", [System.Text.UTF8Encoding]::new($false))
}

function Invoke-Verifier {
    $start = [System.Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @('-NoProfile', '-File', $verifier, '-Directory', $testDirectory, '-ExpectedFileName', $expectedFileName)) {
        $start.ArgumentList.Add($argument)
    }
    $process = [System.Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $result = @{
        Code = $process.ExitCode
        Output = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
    }
    $process.Dispose()
    return $result
}

try {
    New-Item -ItemType Directory -Path $testDirectory | Out-Null
    [System.IO.File]::WriteAllText($packagePath, 'synthetic MSIX fixture')
    Write-Checksum
    $valid = Invoke-Verifier
    if ($valid.Code -ne 0 -or $valid.Output -notmatch 'Verified the downloaded Windows package') {
        throw "A valid package and checksum must pass. Verifier returned: $($valid.Output)"
    }

    [System.IO.File]::AppendAllText($packagePath, 'tampered')
    if ((Invoke-Verifier).Code -eq 0) { throw 'A package whose content changed must fail checksum verification.' }
    [System.IO.File]::WriteAllText($packagePath, 'synthetic MSIX fixture')
    Write-Checksum

    [System.IO.File]::WriteAllText((Join-Path $testDirectory 'unexpected.txt'), 'unexpected')
    if ((Invoke-Verifier).Code -eq 0) { throw 'Unexpected downloaded files must be rejected.' }
    Remove-Item -LiteralPath (Join-Path $testDirectory 'unexpected.txt')

    [System.IO.File]::WriteAllText($checksumPath, ('0' * 64) + "  $expectedFileName`n")
    if ((Invoke-Verifier).Code -eq 0) { throw 'An invalid checksum must be rejected.' }

    Write-Checksum
    [System.IO.File]::AppendAllText($checksumPath, [System.IO.File]::ReadAllText($checksumPath))
    if ((Invoke-Verifier).Code -eq 0) { throw 'Duplicate checksum entries must be rejected.' }
} finally {
    $allowedPrefix = Join-Path $tempRoot 'teleark-msix-artifact-'
    if (-not $testDirectory.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a test directory outside the designated temporary directory.'
    }
    if (Test-Path -LiteralPath $testDirectory) { Remove-Item -LiteralPath $testDirectory -Recurse -Force }
}

Write-Output 'Windows MSIX artifact verification passed five synthetic cases.'
