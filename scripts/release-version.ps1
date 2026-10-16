# Resolve the one version used by the executable and every native installer.
param(
    [string]$RefType = $env:GITHUB_REF_TYPE,
    [string]$RefName = $env:GITHUB_REF_NAME,
    [string]$EventName = $env:GITHUB_EVENT_NAME,
    [string]$CommitSha = $env:GITHUB_SHA,
    [string]$OutputFile = $env:GITHUB_OUTPUT
)

$ErrorActionPreference = 'Stop'
$manifest = Join-Path (Split-Path -Parent $PSScriptRoot) 'Cargo.toml'
$metadata_json = cargo metadata --manifest-path $manifest --format-version 1 --no-deps --locked
if ($LASTEXITCODE -ne 0) {
    throw 'Cargo metadata failed while resolving the release version.'
}

$packages = @(($metadata_json | ConvertFrom-Json).packages | Where-Object { $_.name -eq 'teleark-gui' })
if ($packages.Count -ne 1) {
    throw 'Expected exactly one teleark-gui package in Cargo metadata.'
}
$version = [string]$packages[0].version
if ($version -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
    throw "Release packages require a three-component numeric version; Cargo has '$version'."
}

$components = $version.Split('.')
if ([long]$components[0] -gt 255 -or [long]$components[1] -gt 255 -or [long]$components[2] -gt 65535) {
    throw 'Release version components must fit the Windows Installer limit: 255.255.65535.'
}

if ($RefType -eq 'tag' -and $RefName -cne "v$version") {
    throw "Release tag '$RefName' does not match Cargo version '$version'."
}
if ($EventName -eq 'push' -and $RefType -ne 'tag') {
    throw 'Only a matching version tag may publish a release.'
}

$label = $version
if ($EventName -eq 'workflow_dispatch') {
    if ($CommitSha -cnotmatch '^[0-9a-fA-F]{7,40}$') {
        throw 'Manual release builds require a commit SHA for artifact names.'
    }
    $label = "$version-manual-$($CommitSha.Substring(0, 7).ToLowerInvariant())"
}

$lines = "version=$version`nlabel=$label`n"
if ($OutputFile) {
    [System.IO.File]::AppendAllText($OutputFile, $lines, [System.Text.UTF8Encoding]::new($false))
} else {
    Write-Output $lines.TrimEnd()
}
