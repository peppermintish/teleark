# Install and verify the exact Inno Setup compiler used by Windows packaging.
[CmdletBinding()]
param(
    [ValidateSet('6.7.3')]
    [string]$Version = '6.7.3'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'verify-innosetup-compiler.ps1')

if ($PSVersionTable.PSEdition -eq 'Core' -and -not $IsWindows) {
    throw 'scripts/install-innosetup.ps1 must run on Windows.'
}

$pinnedReleases = @{
    '6.7.3' = @{
        Tag = 'is-6_7_3'
        Asset = 'innosetup-6.7.3.exe'
        Sha256 = '9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732'
    }
}

$release = $pinnedReleases[$Version]
if ($null -eq $release) {
    throw "No verified Inno Setup release is pinned for version $Version."
}

$runnerTemp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$downloadDirectory = Join-Path ([System.IO.Path]::GetFullPath($runnerTemp)) "teleark-innosetup-$Version-$([guid]::NewGuid().ToString('N'))"
$downloadPath = Join-Path $downloadDirectory $release.Asset
$downloadUrl = "https://github.com/jrsoftware/issrc/releases/download/$($release.Tag)/$($release.Asset)"
$installDirectory = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6'
$isccPath = Join-Path $installDirectory 'ISCC.exe'

try {
    New-Item -ItemType Directory -Force -Path $downloadDirectory | Out-Null

    Write-Output "Downloading pinned Inno Setup $Version from the official release."
    Invoke-WebRequest -Uri $downloadUrl -OutFile $downloadPath

    Write-Output 'Verifying the Inno Setup installer checksum and Authenticode signature.'
    $actualSha256 = (Get-FileHash -LiteralPath $downloadPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $release.Sha256) {
        throw "Inno Setup $Version checksum mismatch: expected $($release.Sha256), received $actualSha256."
    }

    $signature = Get-AuthenticodeSignature -FilePath $downloadPath
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Inno Setup $Version Authenticode verification failed with status $($signature.Status)."
    }
    $publisher = $signature.SignerCertificate.GetNameInfo(
        [System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName,
        $false
    )
    if ($publisher -ne 'Pyrsys B.V.') {
        throw "Unexpected Inno Setup installer publisher: $publisher."
    }

    Write-Output "Installing Inno Setup $Version silently in $installDirectory."
    $process = Start-Process -FilePath $downloadPath -ArgumentList @(
        '/VERYSILENT',
        '/SUPPRESSMSGBOXES',
        '/NORESTART',
        '/SP-',
        '/CURRENTUSER',
        "/DIR=`"$installDirectory`""
    ) -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Inno Setup $Version installer failed with exit code $($process.ExitCode)."
    }

    if (-not (Test-Path -LiteralPath $isccPath -PathType Leaf)) {
        throw "Inno Setup $Version installed without the expected compiler: $isccPath"
    }
    $fileVersion = Assert-InnoSetupCompilerVersion -Path $isccPath -ExpectedVersion $Version
    $isccDirectory = Split-Path -Parent $isccPath
    $env:PATH = "$isccDirectory;$env:PATH"
    if ($env:GITHUB_PATH) {
        Add-Content -LiteralPath $env:GITHUB_PATH -Value $isccDirectory -Encoding utf8
    }

    Write-Output "Installed and verified Inno Setup $Version ($fileVersion): $isccPath"
} finally {
    if (Test-Path -LiteralPath $downloadDirectory) {
        Remove-Item -LiteralPath $downloadDirectory -Recurse -Force
    }
}
