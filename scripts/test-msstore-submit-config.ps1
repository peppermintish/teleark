# Synthetic configuration cases; no live Store credentials are used.
$ErrorActionPreference = 'Stop'
$checker = Join-Path $PSScriptRoot 'check-msstore-submit-config.ps1'
$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testDirectory = [System.IO.Path]::GetFullPath(
    (Join-Path $tempRoot "teleark-msstore-config-$([guid]::NewGuid().ToString('N'))")
)

$identity = @{
    Name = 'TeleArk.Package'
    Publisher = 'CN=TeleArk Test Publisher'
    DisplayName = 'TeleArk Test'
}
$complete = @{
    Tenant = 'tenant-secret-sentinel-1234'
    ClientId = 'client-id-sentinel-5678'
    ClientSecret = 'client-secret-sentinel-9012'
    Seller = 'seller-id-sentinel-3456'
    Product = '9WZDNCRFJ3Q8'
}
$cases = @(
    @{ Name = 'package preview without submission credentials'; Identity = $identity; Settings = @{}; Require = $false; Pass = $true; Ready = 'false' },
    @{ Name = 'complete version-tag submission'; Identity = $identity; Settings = $complete; Require = $true; Pass = $true; Ready = 'true' },
    @{ Name = 'partial submission configuration'; Identity = $identity; Settings = @{ Tenant = $complete.Tenant; ClientId = $complete.ClientId }; Require = $false; Pass = $false },
    @{ Name = 'missing client secret'; Identity = $identity; Settings = @{ Tenant = $complete.Tenant; ClientId = $complete.ClientId; Seller = $complete.Seller; Product = $complete.Product }; Require = $true; Pass = $false },
    @{ Name = 'missing Store Product ID'; Identity = $identity; Settings = @{ Tenant = $complete.Tenant; ClientId = $complete.ClientId; ClientSecret = $complete.ClientSecret; Seller = $complete.Seller }; Require = $true; Pass = $false },
    @{ Name = 'missing all release submission credentials'; Identity = $identity; Settings = @{}; Require = $true; Pass = $false },
    @{ Name = 'missing package identity value'; Identity = @{ Name = $identity.Name; Publisher = $identity.Publisher }; Settings = @{}; Require = $false; Pass = $false },
    @{ Name = 'malformed package identity name'; Identity = @{ Name = 'invalid identity'; Publisher = $identity.Publisher; DisplayName = $identity.DisplayName }; Settings = @{}; Require = $false; Pass = $false }
)

$environmentNames = @(
    'TELEARK_MSIX_IDENTITY_NAME',
    'TELEARK_MSIX_PUBLISHER',
    'TELEARK_MSIX_PUBLISHER_DISPLAY_NAME',
    'AZURE_AD_TENANT_ID',
    'AZURE_AD_APPLICATION_CLIENT_ID',
    'AZURE_AD_APPLICATION_SECRET',
    'SELLER_ID',
    'TELEARK_MSSTORE_PRODUCT_ID'
)
$valueMap = @{
    TELEARK_MSIX_IDENTITY_NAME = 'Name'
    TELEARK_MSIX_PUBLISHER = 'Publisher'
    TELEARK_MSIX_PUBLISHER_DISPLAY_NAME = 'DisplayName'
    AZURE_AD_TENANT_ID = 'Tenant'
    AZURE_AD_APPLICATION_CLIENT_ID = 'ClientId'
    AZURE_AD_APPLICATION_SECRET = 'ClientSecret'
    SELLER_ID = 'Seller'
    TELEARK_MSSTORE_PRODUCT_ID = 'Product'
}

try {
    New-Item -ItemType Directory -Path $testDirectory | Out-Null
    foreach ($case in $cases) {
        $caseName = $case.Name -replace '[^A-Za-z0-9]+', '-'
        $outputPath = Join-Path $testDirectory "$caseName.output"
        $summaryPath = Join-Path $testDirectory "$caseName.summary"
        $start = [System.Diagnostics.ProcessStartInfo]::new((Get-Process -Id $PID).Path)
        $start.UseShellExecute = $false
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        foreach ($argument in @('-NoProfile', '-File', $checker)) { $start.ArgumentList.Add($argument) }
        if ($case.Require) { $start.ArgumentList.Add('-RequireSubmission') }
        $start.Environment['GITHUB_OUTPUT'] = $outputPath
        $start.Environment['GITHUB_STEP_SUMMARY'] = $summaryPath
        foreach ($name in $environmentNames) { $start.Environment[$name] = '' }

        foreach ($name in $environmentNames) {
            $key = $valueMap[$name]
            if ($case.Identity.ContainsKey($key)) { $start.Environment[$name] = $case.Identity[$key] }
            if ($case.Settings.ContainsKey($key)) { $start.Environment[$name] = $case.Settings[$key] }
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
            if ($stepOutput -notmatch "(?m)^submission_ready=$($case.Ready)$") {
                throw "Microsoft Store configuration case '$($case.Name)' returned an unexpected readiness result."
            }
            $output += $stepOutput + [System.IO.File]::ReadAllText($summaryPath)
        }
        foreach ($value in @($case.Settings.Values)) {
            if ($value -and $output.Contains($value)) {
                throw "Microsoft Store configuration case '$($case.Name)' exposed a configured value."
            }
        }
    }
} finally {
    $allowedPrefix = Join-Path $tempRoot 'teleark-msstore-config-'
    if (-not $testDirectory.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a test directory outside the designated temporary directory.'
    }
    if (Test-Path -LiteralPath $testDirectory) { Remove-Item -LiteralPath $testDirectory -Recurse -Force }
}

Write-Output "Microsoft Store configuration passed $($cases.Count) synthetic cases with value redaction."
