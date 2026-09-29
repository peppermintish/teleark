param([switch]$RequireSubmission)

$ErrorActionPreference = 'Stop'

$identitySettings = @(
    $env:TELEARK_MSIX_IDENTITY_NAME,
    $env:TELEARK_MSIX_PUBLISHER,
    $env:TELEARK_MSIX_PUBLISHER_DISPLAY_NAME
)
$configuredIdentity = @($identitySettings | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
if ($configuredIdentity.Count -ne $identitySettings.Count) {
    throw 'Configure TELEARK_MSIX_IDENTITY_NAME, TELEARK_MSIX_PUBLISHER, and TELEARK_MSIX_PUBLISHER_DISPLAY_NAME with the exact Partner Center identity.'
}

if ($env:TELEARK_MSIX_IDENTITY_NAME -cnotmatch '^[A-Za-z0-9][A-Za-z0-9.-]{1,48}[A-Za-z0-9]$') {
    throw 'TELEARK_MSIX_IDENTITY_NAME is not a valid package identity name.'
}
if ($env:TELEARK_MSIX_PUBLISHER_DISPLAY_NAME.Length -gt 256) {
    throw 'TELEARK_MSIX_PUBLISHER_DISPLAY_NAME exceeds the 256-character manifest limit.'
}

$submissionSettings = @(
    $env:AZURE_AD_TENANT_ID,
    $env:AZURE_AD_APPLICATION_CLIENT_ID,
    $env:SELLER_ID,
    $env:TELEARK_MSSTORE_PRODUCT_ID
)
$configuredSubmission = @($submissionSettings | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
if ($configuredSubmission.Count -ne 0 -and $configuredSubmission.Count -ne $submissionSettings.Count) {
    throw 'Configure AZURE_AD_TENANT_ID, AZURE_AD_APPLICATION_CLIENT_ID, SELLER_ID, and TELEARK_MSSTORE_PRODUCT_ID together.'
}
if ($RequireSubmission -and $configuredSubmission.Count -ne $submissionSettings.Count) {
    throw 'Version-tag releases require AZURE_AD_TENANT_ID, AZURE_AD_APPLICATION_CLIENT_ID, SELLER_ID, and TELEARK_MSSTORE_PRODUCT_ID.'
}

$submissionReady = $configuredSubmission.Count -eq $submissionSettings.Count
$outputValue = if ($submissionReady) { 'true' } else { 'false' }
if ($env:GITHUB_OUTPUT) {
    [System.IO.File]::AppendAllText($env:GITHUB_OUTPUT, "submission_ready=$outputValue`n")
}

$status = if ($submissionReady) {
    'Store submission configuration is complete.'
} elseif ($RequireSubmission) {
    'Store submission configuration is incomplete.'
} else {
    'Package identity is configured; submission credentials are not required for a manual package preview.'
}
if ($env:GITHUB_STEP_SUMMARY) {
    [System.IO.File]::AppendAllText(
        $env:GITHUB_STEP_SUMMARY,
        "### Microsoft Store configuration`n`n$status Values are never included in this report.`n"
    )
}
Write-Output $status
