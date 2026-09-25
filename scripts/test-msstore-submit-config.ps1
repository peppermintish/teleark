# Synthetic repository configuration cases; no live Store credentials are used.
$ErrorActionPreference = 'Stop'
$checker = Join-Path $PSScriptRoot 'check-msstore-submit-config.ps1'
$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testDirectory = [System.IO.Path]::GetFullPath(
    (Join-Path $tempRoot "teleark-msstore-config-$([guid]::NewGuid().ToString('N'))")
)

$complete = @{
    Tenant = 'tenant-secret-sentinel-1234'
    ClientId = 'client-id-sentinel-5678'
    ClientSecret = 'client-secret-sentinel-9012'
    Seller = 'seller-id-sentinel-3456'
    Product = '9WZDNCRFJ3Q8'
}
$cases = @(
    @{ Name = 'no Store API settings'; Settings = @{}; Msix = 'true'; Pass = $true; Enabled = 'false' },
    @{ Name = 'complete Store API settings'; Settings = $complete; Msix = 'true'; Pass = $true; Enabled = 'true' },
    @{ Name = 'partial authentication settings'; Settings = @{ Tenant = $complete.Tenant; ClientId = $complete.ClientId; ClientSecret = $complete.ClientSecret }; Msix = 'true'; Pass = $false },
    @{ Name = 'missing client secret'; Settings = @{ Tenant = $complete.Tenant; ClientId = $complete.ClientId; Seller = $complete.Seller; Product = $complete.Product }; Msix = 'true'; Pass = $false },
    @{ Name = 'missing Store Product ID'; Settings = @{ Tenant = $complete.Tenant; ClientId = $complete.ClientId; ClientSecret = $complete.ClientSecret; Seller = $complete.Seller }; Msix = 'true'; Pass = $false },
    @{ Name = 'submission settings without MSIX identity'; Settings = $complete; Msix = 'false'; Pass = $false }
)
$environmentNames = @(
    'AZURE_AD_TENANT_ID',
    'AZURE_AD_APPLICATION_CLIENT_ID',
    'AZURE_AD_APPLICATION_SECRET',
    'SELLER_ID',
    'TELEARK_MSSTORE_PRODUCT_ID'
)

try {
    New-Item -ItemType Directory -Path $testDirectory | Out-Null
    foreach ($case in $cases) {
        $outputPath = Join-Path $testDirectory "$($case.Name -replace '[^A-Za-z0-9]+', '-').output"
        $summaryPath = Join-Path $testDirectory "$($case.Name -replace '[^A-Za-z0-9]+', '-').summary"
        $start = [System.Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        foreach ($argument in @('-NoProfile', '-File', $checker)) { $start.ArgumentList.Add($argument) }
        $start.Environment['TELEARK_MSIX_ENABLED'] = $case.Msix
        $start.Environment['GITHUB_OUTPUT'] = $outputPath
        $start.Environment['GITHUB_STEP_SUMMARY'] = $summaryPath
        foreach ($environmentName in $environmentNames) {
            $start.Environment[$environmentName] = ''
        }
        $valueMap = @{
            AZURE_AD_TENANT_ID = 'Tenant'
            AZURE_AD_APPLICATION_CLIENT_ID = 'ClientId'
            AZURE_AD_APPLICATION_SECRET = 'ClientSecret'
            SELLER_ID = 'Seller'
            TELEARK_MSSTORE_PRODUCT_ID = 'Product'
        }
        foreach ($environmentName in $environmentNames) {
            $settingKey = $valueMap[$environmentName]
            if ($case.Settings.ContainsKey($settingKey)) {
                $start.Environment[$environmentName] = $case.Settings[$settingKey]
            }
        }

        $process = [System.Diagnostics.Process]::Start($start)
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $output = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
        $exitCode = $process.ExitCode
        $process.Dispose()
        if (($exitCode -eq 0) -ne $case.Pass) {
            throw "Microsoft Store configuration case '$($case.Name)' returned an unexpected exit code."
        }

        if ($case.Pass) {
            $stepOutput = [System.IO.File]::ReadAllText($outputPath)
            if ($stepOutput -notmatch "(?m)^enabled=$($case.Enabled)$") {
                throw "Microsoft Store configuration case '$($case.Name)' returned an unexpected enabled output."
            }
            $output += $stepOutput + [System.IO.File]::ReadAllText($summaryPath)
        }
        foreach ($secretName in @('Tenant', 'ClientId', 'ClientSecret', 'Seller')) {
            $secretValue = $case.Settings[$secretName]
            if ($secretValue -and $output.Contains($secretValue)) {
                throw "Microsoft Store configuration case '$($case.Name)' exposed a configured secret value."
            }
        }
    }
} finally {
    $allowedPrefix = Join-Path $tempRoot 'teleark-msstore-config-'
    if (-not $testDirectory.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a test directory outside the designated temporary directory.'
    }
    if (Test-Path -LiteralPath $testDirectory) {
        Remove-Item -LiteralPath $testDirectory -Recurse -Force
    }
}

Write-Output "Microsoft Store submission configuration passed $($cases.Count) synthetic cases, including secret redaction."
exit 0
