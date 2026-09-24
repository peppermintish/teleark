# Build an unsigned Store-submission MSIX and its .msixupload container.
# MSIX Store submissions are signed by Microsoft after certification. The optional
# test-signed copy is separate and is never placed in the Store upload container.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$BinaryPath,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$OutputDirectory,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$CargoVersion,
    [Parameter(Mandatory)][ValidatePattern('^[A-Za-z0-9][A-Za-z0-9.-]{1,48}[A-Za-z0-9]$')][string]$IdentityName,
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$Publisher,
    [Parameter(Mandatory)][ValidateLength(1, 256)][string]$PublisherDisplayName,
    [string]$PublicSymbolsPath,
    [switch]$DryRun,
    [switch]$CreateTestSignedPackage,
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
    # Store package versions use Major.Minor.Build.Revision; Revision must be 0,
    # and Major must be nonzero. Offset Cargo's major by one so Cargo 0.x works
    # while preserving a monotonic, one-to-one mapping of the Cargo triplet.
    if ($parts[0] -ge 65535 -or $parts[1] -gt 65535 -or $parts[2] -gt 65535) {
        throw "Cargo version $Version cannot be represented by the Store version mapping; mapped components must be <= 65535."
    }

    $mappedMajor = $parts[0] + 1
    return [pscustomobject]@{
        CargoVersion = $Version
        PackageVersion = '{0}.{1}.{2}.0' -f $mappedMajor, $parts[1], $parts[2]
    }
}

function Get-SdkToolPath {
    param([Parameter(Mandatory)][ValidateSet('MakeAppx.exe', 'SignTool.exe')][string]$ToolName)

    $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    if (-not (Test-Path -LiteralPath $sdkRoot -PathType Container)) {
        throw "Windows SDK tools were not found under '$sdkRoot'. Install the Windows 10 SDK."
    }

    $candidates = @(Get-ChildItem -LiteralPath $sdkRoot -Directory | ForEach-Object {
        $version = $null
        if ([Version]::TryParse($_.Name, [ref]$version)) {
            $candidate = Join-Path (Join-Path $_.FullName 'x64') $ToolName
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                [pscustomobject]@{ Version = $version; Path = $candidate }
            }
        }
    } | Sort-Object Version -Descending)

    if ($candidates.Count -eq 0) {
        throw "Could not find x64 $ToolName under '$sdkRoot'. Install the Windows 10 SDK."
    }
    return $candidates[0].Path
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
        return $reader.ReadUInt16()
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Add-FileToZip {
    param(
        [Parameter(Mandatory)][System.IO.Compression.ZipArchive]$Archive,
        [Parameter(Mandatory)][string]$SourcePath,
        [Parameter(Mandatory)][string]$EntryName,
        [System.IO.Compression.CompressionLevel]$CompressionLevel = [System.IO.Compression.CompressionLevel]::NoCompression
    )

    $entry = $Archive.CreateEntry($EntryName, $CompressionLevel)
    $inputStream = [System.IO.File]::OpenRead($SourcePath)
    $entryStream = $entry.Open()
    try { $inputStream.CopyTo($entryStream) }
    finally {
        $entryStream.Dispose()
        $inputStream.Dispose()
    }
}

function New-AppxSymbolsArchive {
    param(
        [Parameter(Mandatory)][string]$PdbPath,
        [Parameter(Mandatory)][string]$OutputPath
    )

    $archive = [System.IO.Compression.ZipFile]::Open($OutputPath, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        Add-FileToZip -Archive $archive -SourcePath $PdbPath -EntryName 'teleark.pdb' -CompressionLevel ([System.IO.Compression.CompressionLevel]::Optimal)
    } finally {
        $archive.Dispose()
    }
}

function Assert-AppxSymbolsArchive {
    param([Parameter(Mandatory)][string]$Path)

    $archive = [System.IO.Compression.ZipFile]::OpenRead($Path)
    try {
        if (-not @($archive.Entries | Where-Object { $_.Name -match '\.pdb$' }).Count) {
            throw "The .appxsym file has no PDB entry: $Path"
        }
    } finally {
        $archive.Dispose()
    }
}

function New-TestSignedCopy {
    param(
        [Parameter(Mandatory)][string]$InputPackage,
        [Parameter(Mandatory)][string]$OutputPackage,
        [Parameter(Mandatory)][string]$CertificateOutput,
        [Parameter(Mandatory)][string]$PublisherName,
        [Parameter(Mandatory)][string]$SignToolPath
    )

    $certificate = $null
    try {
        $certificate = New-SelfSignedCertificate `
            -Type Custom `
            -Subject $PublisherName `
            -KeyUsage DigitalSignature `
            -KeyAlgorithm RSA `
            -KeyLength 2048 `
            -HashAlgorithm SHA256 `
            -CertStoreLocation 'Cert:\CurrentUser\My' `
            -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}') `
            -NotAfter (Get-Date).AddDays(7) `
            -FriendlyName "TeleArk temporary MSIX test certificate $([Guid]::NewGuid().ToString('N'))"

        Copy-Item -LiteralPath $InputPackage -Destination $OutputPackage
        & $SignToolPath sign /fd SHA256 /sha1 $certificate.Thumbprint /s My $OutputPackage
        if ($LASTEXITCODE -ne 0) { throw "SignTool failed to sign the local test copy (exit code $LASTEXITCODE)." }
        Export-Certificate -Cert $certificate -FilePath $CertificateOutput -Type CERT -Force | Out-Null
    } finally {
        if ($null -ne $certificate) {
            Remove-Item -LiteralPath "Cert:\CurrentUser\My\$($certificate.Thumbprint)" -Force -ErrorAction SilentlyContinue
        }
    }
}

$versionInfo = ConvertTo-StorePackageVersion -Version $CargoVersion
if ([string]::IsNullOrWhiteSpace($PublisherDisplayName)) {
    throw 'PublisherDisplayName must be the exact non-empty display name assigned in Partner Center.'
}
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$packageAssets = Join-Path $repositoryRoot 'packaging\windows\msix\Assets'
$manifestTemplatePath = Join-Path $repositoryRoot 'packaging\windows\msix\AppxManifest.xml.template'
$assetFiles = @('StoreLogo.png', 'Square44x44Logo.png', 'Square150x150Logo.png')
$licenseFiles = @('README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD_PARTY_NOTICES.md')
$binaryPath = [System.IO.Path]::GetFullPath($BinaryPath)
$outputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
$baseName = "TeleArk-$CargoVersion-windows-x86_64"
$msixPath = Join-Path $outputDirectory "$baseName.msix"
$uploadPath = Join-Path $outputDirectory "$baseName.msixupload"
$testDirectory = Join-Path $outputDirectory 'test-signing'
$testPackagePath = Join-Path $testDirectory "$baseName.testsigned.msix"
$testCertificatePath = Join-Path $testDirectory "$baseName-test.cer"

if (-not (Test-Path -LiteralPath $manifestTemplatePath -PathType Leaf)) {
    throw "MSIX manifest template not found: $manifestTemplatePath"
}
foreach ($assetName in $assetFiles) {
    $assetPath = Join-Path $packageAssets $assetName
    if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf)) { throw "Required MSIX logo is missing: $assetPath" }
}
foreach ($licenseName in $licenseFiles) {
    $licensePath = Join-Path $repositoryRoot $licenseName
    if (-not (Test-Path -LiteralPath $licensePath -PathType Leaf)) { throw "Required distribution notice is missing: $licensePath" }
}

$xmlEscape = [System.Security.SecurityElement]::Escape
$renderedManifest = (Get-Content -LiteralPath $manifestTemplatePath -Raw).
    Replace('{{IdentityName}}', $xmlEscape.Invoke($IdentityName)).
    Replace('{{Publisher}}', $xmlEscape.Invoke($Publisher)).
    Replace('{{PublisherDisplayName}}', $xmlEscape.Invoke($PublisherDisplayName)).
    Replace('{{PackageVersion}}', $versionInfo.PackageVersion)
if ($renderedManifest -match '\{\{[^}]+\}\}') { throw 'MSIX manifest contains an unexpanded template placeholder.' }
try { [void][xml]$renderedManifest }
catch { throw "Rendered MSIX manifest is not valid XML: $($_.Exception.Message)" }

$symbolPath = $null
if (-not [string]::IsNullOrWhiteSpace($PublicSymbolsPath)) {
    $symbolPath = [System.IO.Path]::GetFullPath($PublicSymbolsPath)
    if (-not (Test-Path -LiteralPath $symbolPath -PathType Leaf)) { throw "Public symbol file not found: $symbolPath" }
    if ([System.IO.Path]::GetExtension($symbolPath) -notin @('.pdb', '.appxsym')) {
        throw 'PublicSymbolsPath must point to a public .pdb or a prebuilt .appxsym file.'
    }
}

if ($DryRun) {
    Write-Output 'Validated MSIX Store package inputs:'
    Write-Output "  Binary: $binaryPath (x64 PE required when packaging)"
    Write-Output "  Cargo version: $($versionInfo.CargoVersion)"
    Write-Output "  Package version: $($versionInfo.PackageVersion) (Cargo major + 1, minor, patch, 0)"
    Write-Output "  Identity Name: $IdentityName"
    Write-Output "  Publisher: $Publisher"
    Write-Output "  Publisher display name: $PublisherDisplayName"
    Write-Output "  MSIX: $msixPath"
    Write-Output "  Store upload: $uploadPath"
    Write-Output "  Public symbols: $(if ($symbolPath) { $symbolPath } else { 'not supplied' })"
    Write-Output "  Test-signed copy: $(if ($CreateTestSignedPackage) { $testPackagePath } else { 'not requested' })"
    Write-Output '  Package payload includes README, MIT/Apache licenses, third-party notices, and the x64 executable.'
    Write-Output '  Store upload is unsigned; Microsoft signs Store MSIX packages after certification.'
    exit 0
}

if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) { throw "Windows executable not found: $binaryPath" }
if ((Get-PeMachine -Path $binaryPath) -ne 0x8664) { throw 'Windows executable must be x64 (PE machine 0x8664).' }

$makeAppx = Get-SdkToolPath -ToolName 'MakeAppx.exe'
$signTool = if ($CreateTestSignedPackage) { Get-SdkToolPath -ToolName 'SignTool.exe' } else { $null }
$plannedOutputs = @($msixPath, $uploadPath)
if ($CreateTestSignedPackage) { $plannedOutputs += @($testPackagePath, $testCertificatePath) }
$existingOutputs = @($plannedOutputs | Where-Object { Test-Path -LiteralPath $_ })
if ($existingOutputs.Count -gt 0 -and -not $Force) {
    throw "Output already exists; pass -Force to replace these generated files: $($existingOutputs -join ', ')"
}

if (-not [string]::IsNullOrWhiteSpace($symbolPath) -and [System.IO.Path]::GetExtension($symbolPath) -ieq '.appxsym') {
    Assert-AppxSymbolsArchive -Path $symbolPath
}

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
    [System.IO.File]::WriteAllText((Join-Path $contentDirectory 'AppxManifest.xml'), $renderedManifest, [System.Text.UTF8Encoding]::new($false))

    $workMsix = Join-Path $workDirectory "$baseName.msix"
    & $makeAppx pack /o /d $contentDirectory /p $workMsix
    if ($LASTEXITCODE -ne 0) { throw "MakeAppx failed to create the Store package (exit code $LASTEXITCODE)." }
    if (-not (Test-Path -LiteralPath $workMsix -PathType Leaf) -or (Get-Item -LiteralPath $workMsix).Length -eq 0) {
        throw 'MakeAppx did not create a non-empty MSIX package.'
    }

    $uploadSymbols = $null
    if ($symbolPath) {
        if ([System.IO.Path]::GetExtension($symbolPath) -ieq '.pdb') {
            $uploadSymbols = Join-Path $workDirectory 'teleark.appxsym'
            New-AppxSymbolsArchive -PdbPath $symbolPath -OutputPath $uploadSymbols
        } else {
            $uploadSymbols = $symbolPath
        }
    }

    $workUpload = Join-Path $workDirectory "$baseName.msixupload"
    $uploadArchive = [System.IO.Compression.ZipFile]::Open($workUpload, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        Add-FileToZip -Archive $uploadArchive -SourcePath $workMsix -EntryName (Split-Path -Leaf $workMsix)
        if ($uploadSymbols) {
            Add-FileToZip -Archive $uploadArchive -SourcePath $uploadSymbols -EntryName (Split-Path -Leaf $uploadSymbols)
        }
    } finally {
        $uploadArchive.Dispose()
    }

    $workTestPackage = $null
    $workTestCertificate = $null
    if ($CreateTestSignedPackage) {
        $testWorkDirectory = Join-Path $workDirectory 'test-signing'
        New-Item -ItemType Directory -Path $testWorkDirectory | Out-Null
        $workTestPackage = Join-Path $testWorkDirectory "$baseName.testsigned.msix"
        $workTestCertificate = Join-Path $testWorkDirectory "$baseName-test.cer"
        New-TestSignedCopy `
            -InputPackage $workMsix `
            -OutputPackage $workTestPackage `
            -CertificateOutput $workTestCertificate `
            -PublisherName $Publisher `
            -SignToolPath $signTool
    }

    foreach ($outputPath in $plannedOutputs) {
        if (Test-Path -LiteralPath $outputPath) { Remove-Item -LiteralPath $outputPath -Force }
    }
    Move-Item -LiteralPath $workMsix -Destination $msixPath -Force
    Move-Item -LiteralPath $workUpload -Destination $uploadPath -Force
    if ($CreateTestSignedPackage) {
        New-Item -ItemType Directory -Force -Path $testDirectory | Out-Null
        Move-Item -LiteralPath $workTestPackage -Destination $testPackagePath -Force
        Move-Item -LiteralPath $workTestCertificate -Destination $testCertificatePath -Force
    }

    $finalArchive = [System.IO.Compression.ZipFile]::OpenRead($uploadPath)
    try {
        $entryNames = @($finalArchive.Entries | ForEach-Object { $_.FullName })
        if ((Split-Path -Leaf $msixPath) -notin $entryNames) { throw '.msixupload is missing the MSIX package.' }
        if ($symbolPath -and -not @($entryNames | Where-Object { $_ -match '\.appxsym$' }).Count) {
            throw '.msixupload is missing the supplied public symbols.'
        }
    } finally {
        $finalArchive.Dispose()
    }

    $checksumPath = Join-Path $outputDirectory 'SHA256SUMS'
    $uploadName = Split-Path -Leaf $uploadPath
    $checksumLines = @()
    if (Test-Path -LiteralPath $checksumPath -PathType Leaf) {
        foreach ($line in Get-Content -LiteralPath $checksumPath) {
            $fields = $line -split '\s+', 2
            if ($fields.Count -ne 2 -or $fields[1] -cne $uploadName) { $checksumLines += $line }
        }
    }
    $uploadHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $uploadPath).Hash.ToLowerInvariant()
    $checksumLines += "$uploadHash  $uploadName"
    [System.IO.File]::WriteAllLines($checksumPath, [string[]]$checksumLines, [System.Text.UTF8Encoding]::new($false))

    Write-Output "Created unsigned Store package: $msixPath"
    Write-Output "Created Store upload container: $uploadPath"
    Write-Output "Added Store upload checksum to: $checksumPath"
    if ($CreateTestSignedPackage) {
        Write-Output "Created separate test-signed package: $testPackagePath"
        Write-Output "Public test certificate: $testCertificatePath"
        Write-Output 'For local testing, import the .cer into the test user TrustedPeople store, install only the .testsigned.msix, then remove the package and test certificate. Do not submit the test-signed copy or certificate.'
    }
} finally {
    if (Test-Path -LiteralPath $workDirectory) { Remove-Item -LiteralPath $workDirectory -Recurse -Force }
}
