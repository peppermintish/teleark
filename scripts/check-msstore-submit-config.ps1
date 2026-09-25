# Keep Store submission disabled until its full Partner Center configuration exists.
$ErrorActionPreference = 'Stop'

$settings = @(
    $env:AZURE_AD_TENANT_ID,
    $env:AZURE_AD_APPLICATION_CLIENT_ID,
    $env:AZURE_AD_APPLICATION_SECRET,
    $env:SELLER_ID,
    $env:TELEARK_MSSTORE_PRODUCT_ID
)
$configured = @($settings | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })

if ($configured.Count -ne 0 -and $configured.Count -ne $settings.Count) {
    throw 'Configure all four Microsoft Store submission secrets and TELEARK_MSSTORE_PRODUCT_ID, or leave all five unset.'
}

$enabled = $configured.Count -eq $settings.Count
if ($enabled -and $env:TELEARK_MSIX_ENABLED -cne 'true') {
    throw 'Microsoft Store submission settings are configured, but MSIX packaging is disabled. Configure all three TELEARK_MSIX_* repository variables.'
}

$outputValue = if ($enabled) { 'true' } else { 'false' }
if ($env:GITHUB_OUTPUT) {
    [System.IO.File]::AppendAllText($env:GITHUB_OUTPUT, "enabled=$outputValue`n")
}

$status = if ($enabled) { 'Enabled' } else { 'Not configured; upload remains a manual Partner Center step.' }
if ($env:GITHUB_STEP_SUMMARY) {
    [System.IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY,
        "### Microsoft Store submission configuration`n`n$status. Authentication values are never included in this report.`n")
}
Write-Output "Microsoft Store submission configuration: $status"
