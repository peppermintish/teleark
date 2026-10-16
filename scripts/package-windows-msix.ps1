# Package one unsigned MSIX for Microsoft Store submission.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$BinaryPath,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$OutputDirectory,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$CargoVersion,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9][A-Za-z0-9.-]{1,48}[A-Za-z0-9]$')][string]$IdentityName,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Publisher,
    [Parameter(Mandatory)][ValidateLength(1, 256)][string]$PublisherDisplayName,
    [switch]$DryRun,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function ConvertTo-StorePackageVersion {
    param([Parameter(Mandatory)][string]$Version)

    if ($Version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
        throw "Cargo version must be three numeric components without leading zeroes: $Version"
    }

    $parts = @($Version.Split('.') | ForEach-Object { [UInt64]::Parse($_, [Globalization.CultureInfo]::InvariantCulture) })
    if ($parts[0] -ge 65535 -or $parts[1] -gt 65535 -or $parts[2] -gt 65535) {
        throw "Cargo version $Version cannot be represented by the Store package version."
    }

    [pscustomobject]@{
        CargoVersion = $Version
        PackageVersion = '{0}.{1}.{2}.0' -f ($parts[0] + 1), $parts[1], $parts[2]
    }
}

function Get-MakeAppxPath {
    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    if (-not (Test-Path -LiteralPath $sdkRoot -PathType Container)) {
        throw "Windows SDK tools were not found under '$sdkRoot'. Install the Windows 10 SDK."
    }

    $candidates = @(Get-ChildItem -LiteralPath $sdkRoot -Directory | ForEach-Object {
        $version = $null
        if ([Version]::TryParse($_.Name, [ref]$version)) {
            $candidate = Join-Path (Join-Path $_.FullName 'x64') 'MakeAppx.exe'
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                [pscustomobject]@{ Version = $version; Path = $candidate }
            }
        }
    } | Sort-Object Version -Descending)

    if ($candidates.Count -eq 0) {
        throw "Could not find x64 MakeAppx.exe under '$sdkRoot'. Install the Windows 10 SDK."
    }
    $candidates[0].Path
}

function Get-PeMachine {
    param([Parameter(Mandatory)][string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 64) { throw 'Executable is too short to be a valid PE file.' }
        $stream.Position = 0
        if ($reader.ReadUInt16() -ne 0x5a4d) { throw 'Executable does not have a DOS MZ header.' }
        $stream.Position = 0x3c
        $peOffset = $reader.ReadInt32()
        if ($peOffset -lt 64 -or $peOffset -gt ($stream.Length - 6)) { throw 'Executable has an invalid PE header offset.' }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) { throw 'Executable does not have a PE signature.' }
        $reader.ReadUInt16()
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Assert-MsixPackage {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$ExpectedIdentityName,
        [Parameter(Mandatory)][string]$ExpectedPublisher,
        [Parameter(Mandatory)][string]$ExpectedPublisherDisplayName,
        [Parameter(Mandatory)][string]$ExpectedPackageVersion,
        [Parameter(Mandatory)][string[]]$RequiredEntries
    )

    $archive = [System.IO.Compression.ZipFile]::OpenRead($Path)
    try {
        $entries = @{}
        foreach ($entry in $archive.Entries) { $entries[$entry.FullName] = $entry }
        foreach ($entryName in $RequiredEntries) {
            if (-not $entries.ContainsKey($entryName) -or $entries[$entryName].Length -eq 0) {
                throw "MSIX package is missing required content: $entryName"
            }
        }

        $manifestReader = [System.IO.StreamReader]::new($entries['AppxManifest.xml'].Open())
        try { [xml]$manifest = $manifestReader.ReadToEnd() }
        finally { $manifestReader.Dispose() }

        $identity = $manifest.SelectSingleNode("/*[local-name()='Package']/*[local-name()='Identity']")
        $publisherDisplay = $manifest.SelectSingleNode("/*[local-name()='Package']/*[local-name()='Properties']/*[local-name()='PublisherDisplayName']")
        if ($null -eq $identity -or $null -eq $publisherDisplay) {
            throw 'MSIX package manifest is missing identity or publisher display metadata.'
        }
        if ($identity.GetAttribute('Name') -cne $ExpectedIdentityName -or
            $identity.GetAttribute('Publisher') -cne $ExpectedPublisher -or
            $identity.GetAttribute('Version') -cne $ExpectedPackageVersion -or
            $publisherDisplay.InnerText -cne $ExpectedPublisherDisplayName) {
            throw 'MSIX package manifest identity or version does not match the requested Store package.'
        }
    } finally {
        $archive.Dispose()
    }
}

$versionInfo = ConvertTo-StorePackageVersion -Version $CargoVersion
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$packageAssets = Join-Path $repositoryRoot 'packaging\windows\msix\Assets'
$manifestTemplatePath = Join-Path $repositoryRoot 'packaging\windows\msix\AppxManifest.xml.template'
$assetFiles = @('StoreLogo.png', 'Square44x44Logo.png', 'Square150x150Logo.png')
$licenseFiles = @('README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD_PARTY_NOTICES.md')
$binaryPath = [System.IO.Path]::GetFullPath($BinaryPath)
$outputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
$baseName = "TeleArk-$CargoVersion-windows-x86_64"
$msixPath = Join-Path $outputDirectory "$baseName.msix"

if (-not (Test-Path -LiteralPath $manifestTemplatePath -PathType Leaf)) {
    throw "MSIX manifest template not found: $manifestTemplatePath"
}
foreach ($assetName in $assetFiles) {
    if (-not (Test-Path -LiteralPath (Join-Path $packageAssets $assetName) -PathType Leaf)) {
        throw "Required MSIX logo is missing: $assetName"
    }
}
foreach ($licenseName in $licenseFiles) {
    if (-not (Test-Path -LiteralPath (Join-Path $repositoryRoot $licenseName) -PathType Leaf)) {
        throw "Required distribution notice is missing: $licenseName"
    }
}

$renderedManifest = (Get-Content -LiteralPath $manifestTemplatePath -Raw).
    Replace('{{IdentityName}}', [System.Security.SecurityElement]::Escape($IdentityName)).
    Replace('{{Publisher}}', [System.Security.SecurityElement]::Escape($Publisher)).
    Replace('{{PublisherDisplayName}}', [System.Security.SecurityElement]::Escape($PublisherDisplayName)).
    Replace('{{PackageVersion}}', $versionInfo.PackageVersion)
if ($renderedManifest -match '\{\{[^}]+\}\}') { throw 'MSIX manifest contains an unexpanded template placeholder.' }
try { [void][xml]$renderedManifest }
catch { throw "Rendered MSIX manifest is not valid XML: $($_.Exception.Message)" }

if ($DryRun) {
    Write-Output 'Validated Windows Store package inputs.'
    Write-Output "  Cargo version: $CargoVersion"
    Write-Output "  Store package version: $($versionInfo.PackageVersion)"
    Write-Output "  MSIX output: $msixPath"
    exit 0
}

if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) { throw "Windows executable not found: $binaryPath" }
if ((Get-PeMachine -Path $binaryPath) -ne 0x8664) { throw 'Windows executable must be x64 (PE machine 0x8664).' }
if ((Test-Path -LiteralPath $msixPath) -and -not $Force) {
    throw "Output already exists; pass -Force to replace it: $msixPath"
}

$makeAppx = Get-MakeAppxPath
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
$workDirectory = Join-Path $outputDirectory ".teleark-msix-work-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $workDirectory | Out-Null

try {
    $contentDirectory = Join-Path $workDirectory 'content'
    $assetsDirectory = Join-Path $contentDirectory 'Assets'
    New-Item -ItemType Directory -Force -Path $assetsDirectory | Out-Null
    Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $contentDirectory 'teleark.exe')
    foreach ($assetName in $assetFiles) {
        Copy-Item -LiteralPath (Join-Path $packageAssets $assetName) -Destination (Join-Path $assetsDirectory $assetName)
    }
    foreach ($licenseName in $licenseFiles) {
        Copy-Item -LiteralPath (Join-Path $repositoryRoot $licenseName) -Destination (Join-Path $contentDirectory $licenseName)
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $contentDirectory 'AppxManifest.xml'),
        $renderedManifest,
        [System.Text.UTF8Encoding]::new($false)
    )

    $workMsix = Join-Path $workDirectory "$baseName.msix"
    & $makeAppx pack /o /d $contentDirectory /p $workMsix
    if ($LASTEXITCODE -ne 0) { throw "MakeAppx failed to create the Store package (exit code $LASTEXITCODE)." }
    if (-not (Test-Path -LiteralPath $workMsix -PathType Leaf) -or (Get-Item -LiteralPath $workMsix).Length -eq 0) {
        throw 'MakeAppx did not create a non-empty MSIX package.'
    }

    $requiredEntries = @('AppxManifest.xml', 'teleark.exe') +
        @($assetFiles | ForEach-Object { "Assets/$_" }) + $licenseFiles
    Assert-MsixPackage `
        -Path $workMsix `
        -ExpectedIdentityName $IdentityName `
        -ExpectedPublisher $Publisher `
        -ExpectedPublisherDisplayName $PublisherDisplayName `
        -ExpectedPackageVersion $versionInfo.PackageVersion `
        -RequiredEntries $requiredEntries

    if (Test-Path -LiteralPath $msixPath) { Remove-Item -LiteralPath $msixPath -Force }
    Move-Item -LiteralPath $workMsix -Destination $msixPath

    $checksumPath = Join-Path $outputDirectory 'SHA256SUMS'
    $checksumLines = @()
    if (Test-Path -LiteralPath $checksumPath -PathType Leaf) {
        foreach ($line in Get-Content -LiteralPath $checksumPath) {
            $fields = $line -split '\s+', 2
            if ($fields.Count -ne 2 -or $fields[1] -cne (Split-Path -Leaf $msixPath)) { $checksumLines += $line }
        }
    }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $msixPath).Hash.ToLowerInvariant()
    $checksumLines += "$hash  $(Split-Path -Leaf $msixPath)"
    [System.IO.File]::WriteAllLines($checksumPath, [string[]]$checksumLines, [System.Text.UTF8Encoding]::new($false))

    Write-Output "Created unsigned Store package: $msixPath"
    Write-Output "Store package version: $($versionInfo.PackageVersion)"
    Write-Output "Added package checksum to: $checksumPath"
    Write-Output 'The Microsoft Store signs the package after certification.'
} finally {
    if (Test-Path -LiteralPath $workDirectory) { Remove-Item -LiteralPath $workDirectory -Recurse -Force }
}
