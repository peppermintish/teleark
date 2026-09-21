param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Label,
    [Parameter(Mandatory)][ValidateSet('x86_64', 'arm64')][string]$Architecture
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
$expectedMachine = if ($Architecture -eq 'arm64') { 0xaa64 } else { 0x8664 }
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
if ((Get-Item "dist/$setupName").VersionInfo.ProductVersion.Trim() -ne $Version) {
    throw 'Windows installer metadata does not match the application version.'
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

$setup = (Resolve-Path "dist/$setupName").Path
$installDir = Join-Path $temporaryRoot "teleark-install-$([guid]::NewGuid().ToString('N'))"
$registry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{9A67D26D-7281-4FE9-B942-D6D2A8719DF5}_is1'
$runSetup = {
    param($logName)
    $log = Join-Path $temporaryRoot $logName
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$installDir`"", "/LOG=`"$log`"")
    $process = Start-Process -FilePath $setup -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    return @{ ExitCode = $process.ExitCode; Log = $log }
}
try {
    $first = & $runSetup 'teleark-first-install.log'
    if ($first.ExitCode -ne 0) { throw "First Windows installation failed: $($first.ExitCode)" }
    $installed = Join-Path $installDir 'teleark.exe'
    if (-not (Test-Path -LiteralPath $installed -PathType Leaf)) { throw 'Windows installer did not install the executable.' }
    if (-not (Test-Path -LiteralPath $registry)) { throw 'Windows installer did not register the application.' }

    Set-ItemProperty -LiteralPath $registry -Name DisplayVersion -Value '0.0.1'
    $marker = Join-Path $installDir 'preserve-marker.txt'
    [System.IO.File]::WriteAllText($marker, 'preserve')
    $upgrade = & $runSetup 'teleark-upgrade.log'
    if ($upgrade.ExitCode -ne 0) { throw "Windows in-place upgrade failed: $($upgrade.ExitCode)" }
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) { throw 'Windows upgrade removed an unrelated installed file.' }
    if ((Get-ItemProperty -LiteralPath $registry -Name DisplayVersion).DisplayVersion -ne $Version) {
        throw 'Windows upgrade did not update the registered version.'
    }
    if ((Get-FileHash -Algorithm SHA256 $installed).Hash -ne (Get-FileHash -Algorithm SHA256 $standalone).Hash) {
        throw 'Windows upgrade did not put the release executable in place.'
    }

    Set-ItemProperty -LiteralPath $registry -Name DisplayVersion -Value '99.0.0'
    $before = (Get-FileHash -Algorithm SHA256 $installed).Hash
    $downgrade = & $runSetup 'teleark-downgrade.log'
    if ($downgrade.ExitCode -eq 0) { throw 'Windows installer accepted a downgrade.' }
    if ((Get-FileHash -Algorithm SHA256 $installed).Hash -ne $before) { throw 'Windows downgrade changed the installed executable.' }
    if (-not (Select-String -LiteralPath $downgrade.Log -Pattern 'Downgrading is not permitted' -Quiet)) {
        throw 'Windows downgrade log is missing the refusal notification.'
    }
} finally {
    if (Test-Path -LiteralPath $registry) { Set-ItemProperty -LiteralPath $registry -Name DisplayVersion -Value $Version }
    $uninstaller = Join-Path $installDir 'unins000.exe'
    if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
        Start-Process -FilePath $uninstaller -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') -WindowStyle Hidden -Wait | Out-Null
    }
}
Write-Output "Verified Windows $Architecture artifacts, in-place upgrade and downgrade refusal."
