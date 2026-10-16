# Validate protected build inputs without printing their values.
$ErrorActionPreference = 'Stop'
$apiId = 0
$apiIdValid = $env:TELEARK_DISTRIBUTION_TELEGRAM_API_ID -cmatch '^\+?[0-9]+\z' -and
    [int]::TryParse($env:TELEARK_DISTRIBUTION_TELEGRAM_API_ID, [ref]$apiId) -and
    $apiId -gt 0
$apiHashValid = $env:TELEARK_DISTRIBUTION_TELEGRAM_API_HASH -cmatch '^[0-9a-fA-F]{32}\z'
$invalidSettings = @()
if (-not $apiIdValid) { $invalidSettings += 'TELEARK_DISTRIBUTION_TELEGRAM_API_ID' }
if (-not $apiHashValid) { $invalidSettings += 'TELEARK_DISTRIBUTION_TELEGRAM_API_HASH' }
$valid = $invalidSettings.Count -eq 0
$invalidSettingsText = $invalidSettings -join ', '
$status = if ($valid) { 'Passed' } else { 'Failed' }
if ($env:GITHUB_STEP_SUMMARY) {
    [System.IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY,
        "### Telegram distribution configuration`n`n$status. Both build secrets must form a valid API ID/hash pair. Missing or invalid fields: $(if ($valid) { 'none' } else { $invalidSettingsText }). Values are never included in this report.`n")
}
if (-not $valid) {
    throw "Release builds require valid Telegram distribution secrets. Missing or invalid fields: $invalidSettingsText (positive 32-bit ID and 32 hexadecimal hash characters)."
}
Write-Output 'Telegram distribution configuration passed; values withheld.'
