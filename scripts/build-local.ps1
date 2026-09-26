# Build or run with private local configuration, restoring the caller's environment.
$ErrorActionPreference = 'Stop'
Set-PSDebug -Off
$dryRun = $false
$run = $false
foreach ($argument in $args) {
    switch ($argument) {
        { $_ -in '--dry-run', '-DryRun', '-dry-run' } { $dryRun = $true }
        { $_ -in '--run', '-Run', '-run' } { $run = $true }
        default {
            [Console]::Error.WriteLine('Usage: scripts/build-local.ps1 [--run] [--dry-run]')
            exit 2
        }
    }
}

$action = if ($run) { 'run' } else { 'build' }
$cargoArguments = @($action, '--release', '-p', 'teleark-gui', '--bin', 'teleark', '--locked')
if ($dryRun) {
    Write-Output 'Would load and validate repository .env.local (without displaying values).'
    Write-Output "Would execute: cargo $($cargoArguments -join ' ')"
    exit 0
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$configPath = Join-Path $repositoryRoot '.env.local'
$savedEnvironment = @{}
$exitCode = 1
Push-Location -LiteralPath $repositoryRoot
try {
    if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
        throw 'Missing .env.local; copy .env.example and replace its sample values locally.'
    }

    # Restrict the file before reading any credentials. This does not change its contents.
    if ($PSVersionTable.PSEdition -eq 'Desktop' -or $IsWindows) {
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
        $acl = Get-Acl -LiteralPath $configPath
        $acl.SetAccessRuleProtection($true, $false)
        foreach ($rule in @($acl.Access)) { [void]$acl.RemoveAccessRuleSpecific($rule) }
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($identity, 'FullControl', 'Allow'))
        Set-Acl -LiteralPath $configPath -AclObject $acl
    } else {
        [IO.File]::SetUnixFileMode($configPath, [IO.UnixFileMode]::UserRead -bor [IO.UnixFileMode]::UserWrite)
    }

    # Parse simple dotenv assignments without evaluating shell code or displaying values.
    $localEnvironment = @{}
    $lineNumber = 0
    foreach ($line in [IO.File]::ReadAllLines($configPath)) {
        $lineNumber++
        $text = $line.Trim()
        if (-not $text -or $text.StartsWith('#')) { continue }
        if ($text -cnotmatch '^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)\z') {
            throw "Invalid .env.local assignment on line $lineNumber; values were not logged."
        }
        $name = $Matches[1]
        $value = $Matches[2].Trim()
        if ($value.StartsWith('"') -or $value.StartsWith("'")) {
            if ($value.Length -lt 2 -or $value[-1] -ne $value[0]) {
                throw "Unclosed .env.local quote on line $lineNumber; values were not logged."
            }
            $value = $value.Substring(1, $value.Length - 2)
        }
        $localEnvironment[$name] = $value
    }

    # Required values must come from this file; inherited values cannot fill gaps.
    $apiId = 0
    $valid = $localEnvironment['TELEARK_DISTRIBUTION_TELEGRAM_API_ID'] -cmatch '^[1-9][0-9]*\z' -and
        [int]::TryParse($localEnvironment['TELEARK_DISTRIBUTION_TELEGRAM_API_ID'], [ref]$apiId) -and
        $apiId -gt 0 -and $localEnvironment['TELEARK_DISTRIBUTION_TELEGRAM_API_HASH'] -cmatch '^[0-9a-fA-F]{32}\z'
    if (-not $valid) {
        throw '.env.local must define a valid Telegram API ID and Hash; values were not logged.'
    }
    if ($apiId -eq 17349) {
        throw 'Replace the public TEST ONLY sample in .env.local with your own application credentials.'
    }

    foreach ($name in $localEnvironment.Keys) {
        $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, $localEnvironment[$name], 'Process')
    }
    Write-Output "Starting cargo $action with .env.local application identifiers."
    & cargo @cargoArguments
    $exitCode = $LASTEXITCODE
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
} finally {
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
    }
    Pop-Location
}
exit $exitCode
