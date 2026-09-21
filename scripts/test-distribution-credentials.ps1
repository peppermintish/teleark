# Synthetic values only; neither production secrets nor local environment files are needed.
$ErrorActionPreference = 'Stop'
$checker = Join-Path $PSScriptRoot 'check-distribution-credentials.ps1'
$cases = @(
    @{ Id = '12345'; Hash = 'a' * 32; Pass = $true },
    @{ Id = '2147483647'; Hash = 'B' * 32; Pass = $true },
    @{ Id = ''; Hash = ''; Pass = $false },
    @{ Id = '12345'; Hash = ''; Pass = $false },
    @{ Id = ''; Hash = 'a' * 32; Pass = $false },
    @{ Id = '0'; Hash = 'a' * 32; Pass = $false },
    @{ Id = '-1'; Hash = 'a' * 32; Pass = $false },
    @{ Id = '2147483648'; Hash = 'a' * 32; Pass = $false },
    @{ Id = '12345 '; Hash = 'a' * 32; Pass = $false },
    @{ Id = ' 12345'; Hash = 'a' * 32; Pass = $false },
    @{ Id = '12345'; Hash = ('a' * 32) + "`n"; Pass = $false },
    @{ Id = '12345'; Hash = 'g' * 32; Pass = $false },
    @{ Id = '12345'; Hash = 'a' * 31; Pass = $false }
)
$report = Join-Path ([System.IO.Path]::GetTempPath()) "teleark-credentials-$([guid]::NewGuid().ToString('N')).md"
try {
    foreach ($case in $cases) {
        $start = [System.Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        foreach ($argument in @('-NoProfile', '-File', $checker)) { $start.ArgumentList.Add($argument) }
        $start.Environment['TELEARK_DISTRIBUTION_TELEGRAM_API_ID'] = $case.Id
        $start.Environment['TELEARK_DISTRIBUTION_TELEGRAM_API_HASH'] = $case.Hash
        $start.Environment['GITHUB_STEP_SUMMARY'] = $report
        $process = [System.Diagnostics.Process]::Start($start)
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $output = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
        if (($process.ExitCode -eq 0) -ne $case.Pass) { throw 'Credential validation returned an unexpected result.' }
        $process.Dispose()
        $output += [System.IO.File]::ReadAllText($report)
        if ($case.Hash -and $output.Contains($case.Hash)) { throw 'Credential validation exposed a hash.' }
        if ($case.Id.Length -ge 5 -and $output.Contains($case.Id)) { throw 'Credential validation exposed an ID.' }
    }
} finally {
    if (Test-Path -LiteralPath $report) { Remove-Item -LiteralPath $report }
}
Write-Output "Distribution credential validation passed $($cases.Count) synthetic cases, including redaction."
exit 0
