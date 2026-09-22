param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Label,
    [Parameter(Mandatory)][ValidateSet('x86_64')][string]$Architecture,
    [switch]$Isolated
)

$ErrorActionPreference = 'Stop'
$base = "teleark-$Label-windows-$Architecture"
$setupName = "TeleArk-Setup-$Label-windows-$Architecture.exe"
foreach ($name in @("$base.exe", "$base.zip", $setupName, 'SHA256SUMS')) {
    $path = Join-Path dist $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-Item -LiteralPath $path).Length -eq 0) {
        throw "Missing or empty release artifact: $path"
    }
}

$standalone = (Resolve-Path "dist/$base.exe").Path
$reader = [System.IO.BinaryReader]::new([System.IO.File]::OpenRead($standalone))
try {
    $reader.BaseStream.Position = 0x3c
    $peOffset = $reader.ReadInt32()
    $reader.BaseStream.Position = $peOffset + 4
    $machine = $reader.ReadUInt16()
} finally {
    $reader.Dispose()
}
$expectedMachine = 0x8664
if ($machine -ne $expectedMachine) { throw "Windows executable has unexpected PE architecture: 0x$($machine.ToString('x4'))" }

$temporaryRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$unpacked = Join-Path $temporaryRoot ([guid]::NewGuid().ToString('N'))
Expand-Archive -LiteralPath "dist/$base.zip" -DestinationPath $unpacked
foreach ($name in @('teleark.exe', 'LICENSE-MIT', 'LICENSE-APACHE', 'THIRD_PARTY_NOTICES.md')) {
    if (-not (Test-Path -LiteralPath (Join-Path $unpacked $name) -PathType Leaf)) {
        throw "Portable archive is missing $name"
    }
}
if ((Get-FileHash -Algorithm SHA256 $standalone).Hash -ne (Get-FileHash -Algorithm SHA256 (Join-Path $unpacked 'teleark.exe')).Hash) {
    throw 'Standalone and portable Windows executables differ.'
}
if ((Get-Item -LiteralPath $standalone).VersionInfo.ProductVersion.Trim() -ne $Version) {
    throw 'Windows executable metadata does not match the application version.'
}
$lines = @(Get-Content dist/SHA256SUMS)
if ($lines.Count -ne 3) { throw 'Windows checksum manifest must list exactly three artifacts.' }
foreach ($line in $lines) {
    $parts = $line -split '\s+', 2
    $path = Join-Path dist $parts[1]
    if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or (Get-FileHash -Algorithm SHA256 $path).Hash -ine $parts[0]) {
        throw "Windows checksum verification failed for $path"
    }
}

if ((Get-Item -LiteralPath "dist/$setupName").VersionInfo.ProductVersion.Trim() -ne $Version) {
    throw 'EXE installer metadata does not match the application version.'
}
if (-not $Isolated) {
    & "$PSScriptRoot/test-windows-msi.ps1" -InstallerPath 'dist/.windows-installer/TeleArk.msi' -PayloadDir $unpacked -Version $Version
}
& "$PSScriptRoot/test-windows-exe.ps1" -InstallerPath "dist/$setupName" -MsiPath 'dist/.windows-installer/TeleArk.msi' -Isolated:$Isolated
Write-Output "Verified Windows $Architecture standalone executable, portable ZIP and EXE setup wizard."
exit 0
