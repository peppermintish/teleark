# Build a local release using only the trusted, private repository configuration.
$ErrorActionPreference = 'Stop'

$dry_run = $false
$run = $false

foreach ($argument in $args) {
    switch ($argument) {
        { $_ -in '--dry-run', '-DryRun', '-dry-run' } { $dry_run = $true }
        { $_ -in '--run', '-Run', '-run' } { $run = $true }
        default {
            [Console]::Error.WriteLine("Usage: scripts/build-local.ps1 [--run] [--dry-run]")
            exit 2
        }
    }
}

$command = @('cargo', 'build', '--release', '-p', 'teleark-gui', '--bin', 'teleark', '--locked')
if ($run) {
    $command = @('cargo', 'run', '--release', '-p', 'teleark-gui', '--bin', 'teleark', '--locked')
}

if ($dry_run) {
    Write-Output "Would load and validate repository .env.local (without displaying values)."
    Write-Output ("Would execute: " + ($command -join ' '))
    exit 0
}

$repository_root = Split-Path -Parent $PSScriptRoot
Set-Location $repository_root

if (-not (Test-Path .env.local)) {
    [Console]::Error.WriteLine("Missing .env.local; copy .env.example and replace its sample values locally.")
    exit 1
}

# Do not let inherited values fill gaps in the local configuration.
[System.Environment]::SetEnvironmentVariable("TELEARK_DISTRIBUTION_TELEGRAM_API_ID", $null, "Process")
[System.Environment]::SetEnvironmentVariable("TELEARK_DISTRIBUTION_TELEGRAM_API_HASH", $null, "Process")

Get-Content .env.local | ForEach-Object {
    $line = $_.Trim()
    if ($line -and -not $line.StartsWith('#')) {
        $parts = $line.Split('=', 2)
        if ($parts.Length -eq 2) {
            [System.Environment]::SetEnvironmentVariable($parts[0].Trim(), $parts[1].Trim(), "Process")
        }
    }
}

$api_id = [System.Environment]::GetEnvironmentVariable("TELEARK_DISTRIBUTION_TELEGRAM_API_ID", "Process")
$api_hash = [System.Environment]::GetEnvironmentVariable("TELEARK_DISTRIBUTION_TELEGRAM_API_HASH", "Process")

$valid_id = $false
if ($api_id -match '^[1-9][0-9]{0,9}$') {
    $parsed_id = 0
    if ([int64]::TryParse($api_id, [ref]$parsed_id)) {
        if ($parsed_id -le 2147483647) {
            $valid_id = $true
        }
    }
}

$valid_hash = $false
if ($api_hash -and ($api_hash -match '^[0-9a-fA-F]{32}$')) {
    $valid_hash = $true
}

if (-not $valid_id -or -not $valid_hash) {
    [Console]::Error.WriteLine(".env.local must define a valid Telegram API ID and Hash; values were not logged.")
    exit 1
}

if ($api_id -eq '17349') {
    [Console]::Error.WriteLine("Replace the public TEST ONLY sample in .env.local with your own application credentials.")
    exit 1
}

Write-Output "Building release with .env.local application identifiers."
& $command[0] $command[1..($command.Length - 1)]
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
