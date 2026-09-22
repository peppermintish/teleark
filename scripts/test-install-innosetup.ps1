# Exercise the real bootstrap with synthetic files and mocked Windows/download boundaries.
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'This regression requires Windows.' }
$bootstrap = Join-Path $PSScriptRoot 'install-innosetup.ps1'
$stage = Join-Path ([IO.Path]::GetTempPath()) "teleark-inno-test-$([guid]::NewGuid().ToString('N'))"
$savedEnvironment = @{}
foreach ($name in @('RUNNER_TEMP', 'LOCALAPPDATA', 'GITHUB_PATH', 'PATH')) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
}
try {
    New-Item -ItemType Directory -Path $stage | Out-Null
    $env:RUNNER_TEMP = $stage
    $env:LOCALAPPDATA = Join-Path $stage 'Local App Data'
    $env:GITHUB_PATH = Join-Path $stage 'github-path'
    $expectedDirectory = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6'
    $compilerFixture = Join-Path $stage 'compiler.dll'
    Add-Type -OutputAssembly $compilerFixture -TypeDefinition @'
using System.Reflection;
[assembly: AssemblyInformationalVersion("6.7.3")]
public class CompilerFixture {}
'@

    function Invoke-WebRequest {
        param($Uri, $OutFile)
        if ($Uri -ne 'https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-6.7.3.exe') {
            throw "Unexpected installer URL: $Uri"
        }
        [IO.File]::WriteAllText($OutFile, 'synthetic installer')
    }
    function Get-FileHash {
        param($LiteralPath, $Algorithm)
        @{ Hash = if ($case -eq 'checksum') { 'incorrect' } else { '9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732' } }
    }
    function Get-AuthenticodeSignature {
        param($FilePath)
        $certificate = [pscustomobject]@{}
        $certificate | Add-Member ScriptMethod GetNameInfo { param($Type, $Issuer) 'Pyrsys B.V.' }
        @{ Status = if ($case -eq 'signature') { 'NotTrusted' } else { 'Valid' }; SignerCertificate = $certificate }
    }
    function Start-Process {
        param($FilePath, $ArgumentList, [switch]$Wait, [switch]$PassThru)
        $script:installerStarted = $true
        if (-not $Wait -or -not $PassThru -or '/CURRENTUSER' -notin $ArgumentList -or
            "/DIR=`"$expectedDirectory`"" -notin $ArgumentList) {
            throw 'Installation must wait and select the exact per-user directory, preserving spaces.'
        }
        if ($case -eq 'exit') { return @{ ExitCode = 2 } }
        if ($case -ne 'missing') {
            New-Item -ItemType Directory -Force -Path $expectedDirectory | Out-Null
            Copy-Item -LiteralPath $compilerFixture -Destination (Join-Path $expectedDirectory 'ISCC.exe')
        }
        @{ ExitCode = 0 }
    }

    foreach ($case in @('success', 'checksum', 'signature', 'exit', 'missing')) {
        if (Test-Path $expectedDirectory) { Remove-Item -LiteralPath $expectedDirectory -Recurse -Force }
        if (Test-Path $env:GITHUB_PATH) { Remove-Item -LiteralPath $env:GITHUB_PATH }
        $script:installerStarted = $false
        $failure = $null
        try { & $bootstrap } catch { $failure = $_ }
        if ($case -eq 'success') {
            if ($failure) { throw $failure }
            if ((Get-Content -LiteralPath $env:GITHUB_PATH -Raw).Trim() -ne $expectedDirectory) {
                throw 'Bootstrap did not export the installed compiler directory.'
            }
            if (-not $env:PATH.StartsWith("$expectedDirectory;")) { throw 'Current process cannot find the installed compiler.' }
            # Keep a stale PATH entry: later failures must not accept a compiler elsewhere.
        } else {
            if (-not $failure) { throw "Bootstrap accepted the $case failure." }
            if ($case -in @('checksum', 'signature') -and $script:installerStarted) {
                throw 'Unverified installer was executed.'
            }
            if (Test-Path $env:GITHUB_PATH) { throw 'Failed installation was exported to later CI steps.' }
        }
        if (Get-ChildItem -LiteralPath $stage -Directory -Filter 'teleark-innosetup-*') {
            throw 'Bootstrap left a downloaded installer behind.'
        }
    }
} finally {
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name])
    }
    Remove-Item -LiteralPath $stage -Recurse -Force
}
Write-Output 'PASS: exact install path with spaces, PATH export, verification failures and installer failures.'
