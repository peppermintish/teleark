# WiX is a build tool only: the MSI uses Windows Installer's native actions/UI.
param(
    [Parameter(Mandatory)][string]$SourceDir,
    [Parameter(Mandatory)][string]$OutputPath,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
if ($Version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
    throw 'MSI versions must contain three numeric components without leading zeroes.'
}
$parts = $Version.Split('.')
if ([long]$parts[0] -gt 255 -or [long]$parts[1] -gt 255 -or [long]$parts[2] -gt 65535) {
    throw 'MSI version components must fit 255.255.65535.'
}
$SourceDir = (Resolve-Path -LiteralPath $SourceDir).Path
$OutputPath = [System.IO.Path]::GetFullPath($OutputPath)
$previousRollForward = $env:DOTNET_ROLL_FORWARD
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    $env:DOTNET_ROLL_FORWARD = 'Major'
    & dotnet tool run wix -- build "$PSScriptRoot/teleark.wxs" -arch x64 -wx `
        -d "AppVersion=$Version" -d "SourceDir=$SourceDir" `
        -d "IconPath=$PSScriptRoot/../crates/teleark-gui/assets/icons/teleark.ico" -o $OutputPath
    if ($LASTEXITCODE -ne 0) { throw "MSI compilation failed with code $LASTEXITCODE." }
} finally {
    $env:DOTNET_ROLL_FORWARD = $previousRollForward
    Pop-Location
}
if (-not (Test-Path -LiteralPath $OutputPath -PathType Leaf)) {
    throw 'MSI compiler did not create the requested package.'
}
