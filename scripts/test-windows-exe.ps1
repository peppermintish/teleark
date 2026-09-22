# Exercise the shipped wizard code and real Windows Installer with temporary data.
param(
    [Parameter(Mandatory)][string]$InstallerPath,
    [Parameter(Mandatory)][string]$MsiPath,
    [switch]$Isolated
)
$ErrorActionPreference = 'Stop'
$InstallerPath = (Resolve-Path -LiteralPath $InstallerPath).Path
$MsiPath = (Resolve-Path -LiteralPath $MsiPath).Path
$installer = New-Object -ComObject WindowsInstaller.Installer
$temporaryRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$testRoot = Join-Path $temporaryRoot "teleark-exe-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $testRoot | Out-Null
$installDir = Join-Path $testRoot 'installed app'
$installed = Join-Path $installDir 'teleark.exe'
$marker = Join-Path $installDir 'preserve-marker.txt'
$registryKey = 'Software\TeleArk\Installer'
$legacyAppId = '{9A67D26D-7281-4FE9-B942-D6D2A8719DF5}'
$upgradeCode = '{12A5E8E9-C9D4-4B19-A318-9FE2415F4079}'
$productName = 'TeleArk'
$productsToClean = [Collections.Generic.List[string]]::new()

function Invoke-Sql($Database, [string]$Sql) {
    $view = $Database.OpenView($Sql)
    try { [void]$view.Execute() } finally {
        [void]$view.Close()
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
    }
}
function Read-Property([string]$Path, [string]$Name) {
    $database = $installer.OpenDatabase($Path, 0)
    $view = $database.OpenView("SELECT ``Value`` FROM ``Property`` WHERE ``Property`` = '$Name'")
    try {
        [void]$view.Execute()
        return $view.Fetch().StringData(1)
    } finally {
        [void]$view.Close()
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database)
    }
}
function Rewrite-UpgradeRules($Database, [string]$OldVersion = '', [string]$NewVersion = '', [string]$NewCode = '') {
    # MSI does not allow UPDATE to change primary-key columns in the Upgrade table.
    $columns = '`UpgradeCode`, `VersionMin`, `VersionMax`, `Language`, `Attributes`, `Remove`, `ActionProperty`'
    $view = $Database.OpenView("SELECT $columns FROM ``Upgrade``")
    $rows = @()
    try {
        [void]$view.Execute()
        while ($record = $view.Fetch()) {
            $values = @()
            for ($column = 1; $column -le 7; $column++) {
                $value = $record.StringData($column)
                if ($column -eq 1 -and $NewCode) { $value = $NewCode }
                if ($column -in @(2, 3) -and $NewVersion -and $value -eq $OldVersion) { $value = $NewVersion }
                if ($column -eq 5) { $values += [string]$record.IntegerData($column) }
                elseif ($value -eq '') { $values += 'NULL' }
                else { $values += "'$($value.Replace("'", "''"))'" }
            }
            $rows += "INSERT INTO ``Upgrade`` ($columns) VALUES ($($values -join ', '))"
        }
    } finally {
        [void]$view.Close()
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
    }
    Invoke-Sql $Database 'DELETE FROM `Upgrade`'
    foreach ($sql in $rows) { Invoke-Sql $Database $sql }
}
function New-PackageCopy([string]$Source, [string]$Destination, [hashtable]$Properties, [switch]$BreakCabinet) {
    $sourceVersion = Read-Property $Source 'ProductVersion'
    Copy-Item -LiteralPath $Source -Destination $Destination
    $database = $installer.OpenDatabase($Destination, 1)
    try {
        foreach ($name in $Properties.Keys) {
            Invoke-Sql $database "UPDATE ``Property`` SET ``Value`` = '$($Properties[$name])' WHERE ``Property`` = '$name'"
        }
        if ($Properties.ContainsKey('ProductVersion')) {
            Rewrite-UpgradeRules $database -OldVersion $sourceVersion -NewVersion $Properties.ProductVersion
        }
        $summary = $database.SummaryInformation(1)
        $summary.Property(9) = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}'
        [void]$summary.Persist()
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($summary)
        if ($BreakCabinet) {
            Invoke-Sql $database 'UPDATE `Media` SET `Cabinet` = ''#missing-test-cabinet.cab'' WHERE `DiskId` = 1'
        }
        [void]$database.Commit()
    } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
}
function New-Wizard([string]$Package, [string]$Name) {
    $path = Join-Path $testRoot "$Name.exe"
    & "$PSScriptRoot/build-windows-exe.ps1" -MsiPath $Package -OutputPath $path `
        -InstallerRegistryKey $registryKey -LegacyAppId $legacyAppId | Out-Host
    return $path
}
function Invoke-Setup([string]$Path, [string]$Name, [string[]]$Options = @(), [switch]$Reject) {
    $log = Join-Path $testRoot "$Name.log"
    $process = Start-Process -FilePath $Path -WindowStyle Hidden -Wait -PassThru -ArgumentList (
        @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"") + $Options)
    if ($Reject) {
        if ($process.ExitCode -eq 0) { throw "EXE incorrectly reported success in $Name. See $log" }
    } elseif ($process.ExitCode -ne 0) { throw "EXE $Name failed: $($process.ExitCode). See $log" }
    return "$log.msi.log"
}
function Remove-TestProduct([string]$Code) {
    if ($installer.ProductState($Code) -in @(1, 5)) {
        if ($Isolated -and $installer.ProductInfo($Code, 'ProductName') -cne $productName) {
            throw 'Refusing to uninstall a product outside the isolated test namespace.'
        }
        $process = Start-Process msiexec.exe -WindowStyle Hidden -Wait -PassThru -ArgumentList @('/x', $Code, '/qn', '/norestart')
        if ($process.ExitCode -notin @(0, 3010)) { throw "Fixture uninstall failed: $($process.ExitCode)" }
    }
}
function Assert-IsolatedPackage([string]$Path) {
    if ((Read-Property $Path 'ProductName') -cne $productName -or
        (Read-Property $Path 'UpgradeCode') -cne $upgradeCode) {
        throw 'Test package still uses the production identity.'
    }
    $database = $installer.OpenDatabase($Path, 0)
    try {
        foreach ($query in @(
            'SELECT `UpgradeCode` FROM `Upgrade`',
            'SELECT `Key` FROM `Registry`', 'SELECT `Key` FROM `RegLocator`',
            'SELECT `Key` FROM `RemoveRegistry`', 'SELECT `DefaultDir` FROM `Directory`',
            'SELECT `Name` FROM `Shortcut`', 'SELECT `ComponentId` FROM `Component`'
        )) {
            $view = $database.OpenView($query)
            try {
                [void]$view.Execute()
                while ($record = $view.Fetch()) {
                    $value = $record.StringData(1)
                    if (($query -match 'UpgradeCode' -and $value -cne $upgradeCode) -or
                        $value -match 'Software\\TeleArk\\|9A67D26D-7281-4FE9-B942-D6D2A8719DF5|6BA13742-BC89-44B4-82E7-0A368BF3812E|2A16B53C-56F5-4644-9BF2-C7DD4053B72A' -or
                        $value -eq 'TeleArk' -or $value.EndsWith('|TeleArk')) {
                        throw 'Test package can still target production files, registration or upgrades.'
                    }
                }
            } finally {
                [void]$view.Close()
                [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
            }
        }
    } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
}
function Assert-Installed([string]$Code, [string]$Hash) {
    if ($installer.ProductState($Code) -ne 5 -or -not (Test-Path -LiteralPath $installed) -or
        ($Hash -and (Get-FileHash -LiteralPath $installed).Hash -ne $Hash)) {
        throw 'EXE failed to preserve the expected installed product and executable.'
    }
    if (-not (Test-Path -LiteralPath $marker)) { throw 'EXE removed an unrelated file.' }
}

if ($Isolated) {
    # Test copies get an entirely different identity, components, registry namespace,
    # default directory and shortcut. The wizard/transaction code is unchanged.
    $productName = 'TeleArkSetupTest-' + [guid]::NewGuid().ToString('N')
    $registryKey = "Software\$productName\Installer"
    $legacyAppId = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}'
    $upgradeCode = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}'
    $isolatedMsi = Join-Path $testRoot 'current.msi'
    New-PackageCopy $MsiPath $isolatedMsi @{
        ProductName = $productName; UpgradeCode = $upgradeCode
        ProductCode = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}'
    }
    $database = $installer.OpenDatabase($isolatedMsi, 1)
    try {
        # Upgrade searches must be isolated too, never just product registration.
        Rewrite-UpgradeRules $database -NewCode $upgradeCode
        foreach ($table in @(
            @('Registry', 'Registry', 'Key'), @('RegLocator', 'Signature_', 'Key'),
            @('RemoveRegistry', 'RemoveRegistry', 'Key'), @('Component', 'Component', 'ComponentId'),
            @('Directory', 'Directory', 'DefaultDir'), @('Shortcut', 'Shortcut', 'Name')
        )) {
            $view = $database.OpenView("SELECT ``$($table[1])``, ``$($table[2])`` FROM ``$($table[0])``")
            $updates = @()
            try {
                [void]$view.Execute()
                while ($record = $view.Fetch()) {
                    $value = $record.StringData(2)
                    if ($table[0] -eq 'Component') { $value = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}' }
                    else {
                        $value = $value.Replace('Software\TeleArk\Installer', $registryKey).
                            Replace('{9A67D26D-7281-4FE9-B942-D6D2A8719DF5}', $legacyAppId)
                        if ($value -eq 'TeleArk') { $value = $productName }
                    }
                    $updates += "UPDATE ``$($table[0])`` SET ``$($table[2])`` = '$value' WHERE ``$($table[1])`` = '$($record.StringData(1))'"
                }
            } finally {
                [void]$view.Close()
                [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($view)
            }
            foreach ($sql in $updates) { Invoke-Sql $database $sql }
        }
        [void]$database.Commit()
    } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
    # Prove the guard rejects the production package before checking the fixture.
    $rejected = $false
    try { Assert-IsolatedPackage $MsiPath } catch { $rejected = $true }
    if (-not $rejected) { throw 'The isolation guard accepted a production package.' }
    Assert-IsolatedPackage $isolatedMsi
    # Regression: changing only Property.UpgradeCode must never count as isolation.
    $unsafeFixture = Join-Path $testRoot 'rejected-upgrade-lookup.msi'
    New-PackageCopy $isolatedMsi $unsafeFixture @{}
    $database = $installer.OpenDatabase($unsafeFixture, 1)
    try {
        Rewrite-UpgradeRules $database -NewCode '{12A5E8E9-C9D4-4B19-A318-9FE2415F4079}'
        [void]$database.Commit()
    } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) }
    $rejected = $false
    try { Assert-IsolatedPackage $unsafeFixture } catch { $rejected = $true }
    Remove-Item -LiteralPath $unsafeFixture
    if (-not $rejected) { throw 'The isolation guard accepted a production upgrade lookup.' }
    $MsiPath = $isolatedMsi
    $InstallerPath = New-Wizard $MsiPath 'current'
} else {
    $legacyRegistry = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\${legacyAppId}_is1"
    if (@($installer.RelatedProducts($upgradeCode)).Count -ne 0 -or
        (Test-Path -LiteralPath "HKCU:\$registryKey") -or (Test-Path -LiteralPath $legacyRegistry) -or
        (Test-Path -LiteralPath "$env:LOCALAPPDATA/Programs/TeleArk") -or
        (Test-Path -LiteralPath (Join-Path ([Environment]::GetFolderPath('Programs')) 'TeleArk.lnk'))) {
        throw 'EXE tests require a clean user account. Use -Isolated to preserve an existing TeleArk installation.'
    }
}

try {
    $currentProduct = Read-Property $MsiPath 'ProductCode'
    $productsToClean.Add($currentProduct)
    Write-Output 'Testing EXE installation, repair and preserved install location...'
    $null = Invoke-Setup $InstallerPath 'fresh' @("/DIR=`"$installDir`"")
    [IO.File]::WriteAllText($marker, 'preserve')
    $hash = (Get-FileHash -LiteralPath $installed).Hash
    Assert-Installed $currentProduct $hash
    Remove-Item -LiteralPath $installed
    $null = Invoke-Setup $InstallerPath 'repair' @("/DIR=`"$(Join-Path $testRoot 'wrong location')`"")
    Assert-Installed $currentProduct $hash
    Remove-TestProduct $currentProduct
    if ((Test-Path -LiteralPath $installed) -or -not (Test-Path -LiteralPath $marker)) { throw 'Uninstall did not preserve only unrelated files.' }

    $olderMsi = Join-Path $testRoot 'older.msi'
    $olderProduct = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}'
    $productsToClean.Add($olderProduct)
    New-PackageCopy $MsiPath $olderMsi @{ ProductCode = $olderProduct; ProductVersion = '0.0.1' }
    if ($Isolated) { Assert-IsolatedPackage $olderMsi }
    $olderExe = New-Wizard $olderMsi 'older'
    $brokenMsi = Join-Path $testRoot 'broken.msi'
    $brokenProduct = '{' + [guid]::NewGuid().ToString().ToUpperInvariant() + '}'
    $productsToClean.Add($brokenProduct)
    New-PackageCopy $MsiPath $brokenMsi @{ ProductCode = $brokenProduct } -BreakCabinet
    if ($Isolated) { Assert-IsolatedPackage $brokenMsi }
    $brokenExe = New-Wizard $brokenMsi 'broken'

    Write-Output 'Testing EXE upgrade from an existing MSI, failure rollback and downgrade refusal...'
    $process = Start-Process msiexec.exe -WindowStyle Hidden -Wait -PassThru -ArgumentList @(
        '/i', "`"$olderMsi`"", '/qn', '/norestart', "INSTALLFOLDER=`"$installDir`"")
    if ($process.ExitCode -ne 0) { throw 'Could not install the old MSI fixture.' }
    $log = Invoke-Setup $brokenExe 'rollback' -Reject
    if (-not (Select-String -LiteralPath $log -Pattern 'missing-test-cabinet' -Quiet)) { throw 'Failure did not reach the missing cabinet.' }
    Assert-Installed $olderProduct $hash
    $null = Invoke-Setup $InstallerPath 'upgrade'
    Assert-Installed $currentProduct $hash
    if ($installer.ProductState($olderProduct) -eq 5) { throw 'EXE upgrade left a duplicate old product.' }
    $log = Invoke-Setup $olderExe 'downgrade' -Reject
    if (-not (Select-String -LiteralPath $log -Pattern 'Downgrading is not permitted' -Quiet)) { throw 'Downgrade omitted its reason.' }
    Assert-Installed $currentProduct $hash
} finally {
    $productsToClean.Reverse()
    foreach ($code in $productsToClean) { Remove-TestProduct $code }
    [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
}
Write-Output "EXE installation, repair, MSI upgrade, rollback and downgrade refusal passed. Logs: $testRoot"
if ($env:GITHUB_STEP_SUMMARY) {
    [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY,
        "### Windows EXE verification`n`nPassed: install, repair, MSI-to-EXE upgrade, rollback, downgrade refusal, retained directory and unrelated files.`n")
}
