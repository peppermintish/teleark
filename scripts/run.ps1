# Build and launch using the repository's private local configuration.
$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\build-local.ps1" --run @args
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
