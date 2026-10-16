[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$PackagePath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$requiredSettings = @(
    'AZURE_AD_TENANT_ID',
    'AZURE_AD_APPLICATION_CLIENT_ID',
    'AZURE_AD_APPLICATION_SECRET',
    'TELEARK_MSSTORE_PRODUCT_ID'
)
foreach ($setting in $requiredSettings) {
    if ([string]::IsNullOrWhiteSpace([System.Environment]::GetEnvironmentVariable($setting))) {
        throw "Required Microsoft Store submission setting $setting is missing."
    }
}

try {
    python (Join-Path $PSScriptRoot 'msstore_submission.py') submit --package-path $PackagePath
    if ($LASTEXITCODE -ne 0) { throw 'Microsoft Store submission failed; review the sanitized Python report and receipt.' }
} finally {
    [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $null, 'Process')
}
