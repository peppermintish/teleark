# Build a local, unsigned Windows Store package from the private .env.local values.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9][A-Za-z0-9.-]{1,48}[A-Za-z0-9]$')][string]$IdentityName,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Publisher,
    [Parameter(Mandatory)][ValidateLength(1, 256)][string]$PublisherDisplayName,
    [string]$OutputDirectory = 'dist',
    [switch]$DryRun,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:OS -cne 'Windows_NT') { throw 'The local MSIX build requires Windows.' }

$repositoryRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repositoryRoot

$metadata = cargo metadata --no-deps --format-version 1 --locked
if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed while resolving the package version.' }
$packages = @(($metadata | ConvertFrom-Json).packages | Where-Object { $_.name -eq 'teleark-gui' })
if ($packages.Count -ne 1) { throw 'Expected exactly one teleark-gui package in Cargo metadata.' }
$cargoVersion = [string]$packages[0].version

$target = 'x86_64-pc-windows-msvc'
$env:CARGO_BUILD_TARGET = $target
$binaryPath = Join-Path $repositoryRoot "target\$target\release\teleark.exe"
if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $resolvedOutputDirectory = $OutputDirectory
} else {
    $resolvedOutputDirectory = Join-Path $repositoryRoot $OutputDirectory
}

$packageScript = Join-Path $PSScriptRoot 'package-windows-msix.ps1'
$packageParameters = @{
    BinaryPath = $binaryPath
    OutputDirectory = $resolvedOutputDirectory
    CargoVersion = $cargoVersion
    IdentityName = $IdentityName
    Publisher = $Publisher
    PublisherDisplayName = $PublisherDisplayName
}
if ($Force) { $packageParameters.Force = $true }

if ($DryRun) {
    & $packageScript @packageParameters -DryRun
    return
}

& (Join-Path $PSScriptRoot 'build-local.ps1') --msix
if ($LASTEXITCODE -ne 0) { throw "Local MSIX-feature build failed with exit code $LASTEXITCODE." }

& $packageScript @packageParameters
if ($LASTEXITCODE -ne 0) { throw "Local MSIX packaging failed with exit code $LASTEXITCODE." }
