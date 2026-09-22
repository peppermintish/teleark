# Build the visible EXE wizard around the transactional installation payload.
param(
    [Parameter(Mandatory)][string]$MsiPath,
    [Parameter(Mandatory)][string]$OutputPath,
    [string]$InstallerRegistryKey = 'Software\TeleArk\Installer',
    [string]$LegacyAppId = '{9A67D26D-7281-4FE9-B942-D6D2A8719DF5}'
)
$ErrorActionPreference = 'Stop'
$MsiPath = (Resolve-Path -LiteralPath $MsiPath).Path
$OutputPath = [System.IO.Path]::GetFullPath($OutputPath)
$iscc = if ($command = Get-Command ISCC.exe -ErrorAction SilentlyContinue) { $command.Source }
else {
    @(
        "$env:LOCALAPPDATA/Programs/Inno Setup 6/ISCC.exe",
        "${env:ProgramFiles(x86)}/Inno Setup 6/ISCC.exe",
        "$env:ProgramFiles/Inno Setup 6/ISCC.exe"
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (-not $iscc) { throw 'Inno Setup 6.7.3 (ISCC.exe) is required to build the EXE installer.' }

$installer = New-Object -ComObject WindowsInstaller.Installer
$database = $installer.OpenDatabase($MsiPath, 0)
$metadata = @{}
try {
    foreach ($name in @('ProductName', 'ProductVersion', 'ProductCode')) {
        $view = $database.OpenView("SELECT ``Value`` FROM ``Property`` WHERE ``Property`` = '$name'")
        try {
            [void]$view.Execute()
            $metadata[$name] = $view.Fetch().StringData(1)
        } finally {
            [void]$view.Close()
            [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
        }
    }
} finally {
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
}

# Format installer-owned messages with the same Fluent catalogs as the application.
$messageFile = Join-Path (Split-Path -Parent $MsiPath) 'installer-messages.iss'
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    & cargo run --quiet --locked -p teleark-i18n --example installer-messages -- $messageFile
    if ($LASTEXITCODE -ne 0) { throw 'Installer message generation failed.' }
    & $iscc /Qp "/DAppName=$($metadata.ProductName)" "/DAppVersion=$($metadata.ProductVersion)" `
        "/DProductCode=$($metadata.ProductCode)" "/DMsiPath=$MsiPath" "/DMessageFile=$messageFile" `
        "/DInstallerRegistryKey=$InstallerRegistryKey" "/DLegacyAppId=$LegacyAppId" `
        "/DOutputDir=$(Split-Path -Parent $OutputPath)" "/DOutputBaseFilename=$([IO.Path]::GetFileNameWithoutExtension($OutputPath))" `
        "$PSScriptRoot/teleark.iss"
    if ($LASTEXITCODE -ne 0) { throw "EXE installer compilation failed with code $LASTEXITCODE." }
} finally {
    Pop-Location
}
if (-not (Test-Path -LiteralPath $OutputPath -PathType Leaf)) { throw 'EXE installer was not created.' }
