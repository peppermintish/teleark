# Exercise actual Windows Installer transactions using synthetic, temporary data.
param(
    [Parameter(Mandatory)][string]$InstallerPath,
    [Parameter(Mandatory)][string]$PayloadDir,
    [Parameter(Mandatory)][string]$Version
)
$ErrorActionPreference = 'Stop'
$installer = New-Object -ComObject WindowsInstaller.Installer
$upgradeCode = '{12A5E8E9-C9D4-4B19-A318-9FE2415F4079}'
$registry = 'HKCU:\Software\TeleArk\Installer'
$legacyKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{9A67D26D-7281-4FE9-B942-D6D2A8719DF5}_is1'
$legacyRegistry = "HKCU:\$legacyKey"
$currentSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
# Use the native provider so MSIX-hosted development shells and Windows Installer
# see the same fixture. Access is limited to this user's one guarded legacy key.
$legacyProviderArguments = @{ hDefKey = [uint32]2147483651; sSubKeyName = "$currentSid\$legacyKey" }
function Invoke-LegacyRegistry([string]$Method, [hashtable]$Values = @{}, [switch]$AllowMissing) {
    $parameters = $legacyProviderArguments.Clone()
    foreach ($name in $Values.Keys) { $parameters[$name] = $Values[$name] }
    $result = Invoke-CimMethod -Namespace root/default -ClassName StdRegProv -MethodName $Method -Arguments $parameters
    if ($result.ReturnValue -ne 0 -and -not ($AllowMissing -and $result.ReturnValue -eq 2)) {
        throw "Native legacy registry fixture operation failed: $Method, $($result.ReturnValue)"
    }
    return $result
}
function Get-LegacyInstallation {
    $result = Invoke-LegacyRegistry 'EnumValues' -AllowMissing
    if ($result.ReturnValue -eq 2) { return $null }
    $location = Invoke-LegacyRegistry 'GetStringValue' @{ sValueName = 'InstallLocation' } -AllowMissing
    $version = Invoke-LegacyRegistry 'GetStringValue' @{ sValueName = 'DisplayVersion' } -AllowMissing
    return @{ Directory = $location.sValue; Version = $version.sValue }
}
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'TeleArk.lnk'
if (@($installer.RelatedProducts($upgradeCode)).Count -ne 0 -or
    (Test-Path -LiteralPath $registry) -or (Test-Path -LiteralPath $legacyRegistry) -or
    (Get-LegacyInstallation) -or (Test-Path -LiteralPath "HKLM:\$legacyKey") -or
    (Test-Path -LiteralPath $shortcut) -or (Test-Path -LiteralPath "$env:LOCALAPPDATA/Programs/TeleArk")) {
    throw 'Installer tests require a clean user account without an existing TeleArk installation or shortcut.'
}
$InstallerPath = (Resolve-Path -LiteralPath $InstallerPath).Path
$PayloadDir = (Resolve-Path -LiteralPath $PayloadDir).Path
$temporaryRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$testRoot = Join-Path $temporaryRoot "teleark-msi-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $testRoot | Out-Null
$installDir = Join-Path $testRoot 'installed app'
$installed = Join-Path $installDir 'teleark.exe'
$marker = Join-Path $installDir 'preserve-marker.txt'
$expectedHash = (Get-FileHash -LiteralPath "$PayloadDir/teleark.exe").Hash
$productsToClean = [System.Collections.Generic.List[string]]::new()

function Read-MsiProperty([string]$Path, [string]$Name) {
    $database = $installer.OpenDatabase($Path, 0)
    $view = $database.OpenView("SELECT ``Value`` FROM ``Property`` WHERE ``Property`` = '$Name'")
    try {
        [void]$view.Execute()
        $record = $view.Fetch()
        if ($record) { return $record.StringData(1) }
        return ''
    } finally {
        [void]$view.Close()
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
    }
}

function Invoke-Msi([string]$Package, [string]$Name, [string[]]$Options = @(), [switch]$Reject) {
    $log = Join-Path $testRoot "$Name.log"
    $arguments = @('/i', "`"$Package`"", '/qn', '/norestart', '/L*v', "`"$log`"") + $Options
    $process = Start-Process msiexec.exe -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    if ($Reject) {
        if ($process.ExitCode -ne 1603) { throw "Expected refusal in $Name; exit $($process.ExitCode). See $log" }
    } elseif ($process.ExitCode -notin @(0, 3010)) {
        throw "MSI $Name failed: $($process.ExitCode). See $log"
    }
    return $log
}

function Remove-TestProduct([string]$Code) {
    if ($installer.ProductState($Code) -in @(1, 5)) {
        $process = Start-Process msiexec.exe -ArgumentList @('/x', $Code, '/qn', '/norestart') -WindowStyle Hidden -Wait -PassThru
        if ($process.ExitCode -notin @(0, 3010)) { throw "Test MSI uninstall failed: $($process.ExitCode)" }
    }
}

function Assert-Payload {
    if (-not (Test-Path -LiteralPath $installed) -or (Get-FileHash -LiteralPath $installed).Hash -ne $expectedHash) {
        throw 'MSI did not install the release executable at the original location.'
    }
    if (-not (Test-Path -LiteralPath $marker)) { throw 'MSI removed an unrelated file.' }
}

function New-LegacyFixture([string]$FixtureVersion) {
    # Windows' built-in C# compiler creates a tiny versioned PE; it is never executed.
    $source = Join-Path $testRoot 'legacy-fixture.cs'
    $text = "[assembly: System.Reflection.AssemblyFileVersion(`"$FixtureVersion.0`")]`nclass Fixture { static void Main() {} }`n"
    [System.IO.File]::WriteAllText($source, $text)
    & "$env:WINDIR/Microsoft.NET/Framework64/v4.0.30319/csc.exe" /nologo /target:winexe /platform:x64 "/out:$installed" $source
    if ($LASTEXITCODE -ne 0) { throw 'Could not compile the legacy version fixture.' }
    # The shipped Inno installer registered this per-user uninstall key in HKCU.
    $null = Invoke-LegacyRegistry 'CreateKey'
    $null = Invoke-LegacyRegistry 'SetStringValue' @{ sValueName = 'InstallLocation'; sValue = "$installDir\" }
    $null = Invoke-LegacyRegistry 'SetStringValue' @{ sValueName = 'DisplayVersion'; sValue = $FixtureVersion }
    [System.IO.File]::WriteAllText((Join-Path $installDir 'unins000.exe'), 'synthetic legacy uninstaller')
    [System.IO.File]::WriteAllText((Join-Path $installDir 'unins000.dat'), 'synthetic legacy metadata')
}

try {
    $currentProduct = Read-MsiProperty $InstallerPath 'ProductCode'
    if ((Read-MsiProperty $InstallerPath 'ProductVersion') -cne $Version -or
        (Read-MsiProperty $InstallerPath 'UpgradeCode') -ine $upgradeCode) {
        throw 'MSI metadata does not match the release contract.'
    }
    $productsToClean.Add($currentProduct)
    Write-Output 'Testing MSI fresh installation and repair...'
    $null = Invoke-Msi $InstallerPath 'fresh-install' @("INSTALLFOLDER=`"$installDir`"")
    [System.IO.File]::WriteAllText($marker, 'preserve')
    Assert-Payload
    Remove-Item -LiteralPath $installed
    $null = Invoke-Msi $InstallerPath 'repair' @('REINSTALL=ALL', 'REINSTALLMODE=amus')
    Assert-Payload
    Remove-TestProduct $currentProduct
    if (Test-Path -LiteralPath $installed) { throw 'MSI uninstall retained the application executable.' }
    if (-not (Test-Path -LiteralPath $marker)) { throw 'MSI uninstall removed an unrelated file.' }

    $older = Join-Path $testRoot 'older.msi'
    & "$PSScriptRoot/build-windows-msi.ps1" -SourceDir $PayloadDir -OutputPath $older -Version '0.0.1'
    $olderProduct = Read-MsiProperty $older 'ProductCode'
    $productsToClean.Add($olderProduct)
    $null = Invoke-Msi $older 'old-install' @("INSTALLFOLDER=`"$installDir`"")

    # A private MSI copy references a missing embedded cabinet, producing a native
    # file-installation failure without adding any failure hooks to release packages.
    $broken = Join-Path $testRoot 'rollback-fixture.msi'
    Copy-Item -LiteralPath $InstallerPath -Destination $broken
    $database = $installer.OpenDatabase($broken, 1)
    $brokenProduct = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}'
    $productsToClean.Add($brokenProduct)
    $summary = $database.SummaryInformation(1)
    $summary.Property(9) = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}'
    [void]$summary.Persist()
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($summary)
    foreach ($sql in @(
        "UPDATE ``Property`` SET ``Value`` = '$brokenProduct' WHERE ``Property`` = 'ProductCode'",
        'UPDATE `Media` SET `Cabinet` = ''#missing-test-cabinet.cab'' WHERE `DiskId` = 1'
    )) {
        $view = $database.OpenView($sql)
        [void]$view.Execute()
        [void]$view.Close()
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
    }
    [void]$database.Commit()
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)

    Write-Output 'Testing MSI upgrade rollback, successful upgrade and downgrade refusal...'
    $log = Invoke-Msi $broken 'upgrade-rollback' -Reject
    if (-not (Select-String -LiteralPath $log -Pattern 'missing-test-cabinet' -Quiet)) { throw 'Rollback fixture did not reach the missing cabinet.' }
    if ($installer.ProductState($olderProduct) -ne 5) { throw 'Failed upgrade did not restore the previous MSI registration.' }
    Assert-Payload
    $null = Invoke-Msi $InstallerPath 'upgrade'
    if ($installer.ProductState($olderProduct) -eq 5 -or $installer.ProductState($currentProduct) -ne 5) {
        throw 'Major upgrade did not replace the old MSI registration.'
    }
    Assert-Payload
    $log = Invoke-Msi $older 'downgrade' -Reject
    if (-not (Select-String -LiteralPath $log -Pattern 'Downgrading is not permitted' -Quiet)) { throw 'Downgrade refusal omitted its explanation.' }
    Assert-Payload
    Remove-TestProduct $currentProduct

    Write-Output 'Testing legacy installer migration, refusal and rollback...'
    New-LegacyFixture '99.0.0'
    $legacyHash = (Get-FileHash -LiteralPath $installed).Hash
    $log = Invoke-Msi $InstallerPath 'legacy-downgrade' -Reject
    if ((Get-FileHash -LiteralPath $installed).Hash -ne $legacyHash -or
        -not (Select-String -LiteralPath $log -Pattern 'Downgrading is not permitted' -Quiet)) {
        throw 'Legacy downgrade did not preserve the application and explain the refusal.'
    }
    New-LegacyFixture '0.0.1'
    $legacyHash = (Get-FileHash -LiteralPath $installed).Hash
    $log = Invoke-Msi $broken 'legacy-rollback' -Reject
    if (-not (Select-String -LiteralPath $log -Pattern 'missing-test-cabinet' -Quiet) -or
        -not (Get-LegacyInstallation) -or
        -not (Test-Path -LiteralPath (Join-Path $installDir 'unins000.exe')) -or
        (Get-FileHash -LiteralPath $installed).Hash -ne $legacyHash) {
        throw 'Failed migration did not restore the legacy installation.'
    }
    $null = Invoke-Msi $InstallerPath 'legacy-migration'
    Assert-Payload
    if ((Get-LegacyInstallation) -or (Test-Path -LiteralPath (Join-Path $installDir 'unins000.exe'))) {
        throw 'Successful migration retained obsolete legacy uninstall metadata.'
    }
} finally {
    $productsToClean.Reverse()
    foreach ($code in $productsToClean) { Remove-TestProduct $code }
    # Only remove the synthetic legacy registration created under the guarded clean account.
    if ($legacy = Get-LegacyInstallation) {
        $registeredPath = $legacy.Directory
        if ($registeredPath.TrimEnd('\') -ne $installDir) { throw 'Refusing to remove a non-test legacy registration.' }
        $null = Invoke-LegacyRegistry 'DeleteKey'
    }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
}
Write-Output "MSI installation, repair, upgrade, rollback, downgrade refusal and legacy migration passed. Logs: $testRoot"
if ($env:GITHUB_STEP_SUMMARY) {
    [System.IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY,
        "### Windows MSI verification`n`nPassed: fresh install, repair, in-place upgrade, rollback, downgrade refusal, legacy migration and preservation of unrelated files.`n")
}
exit 0
