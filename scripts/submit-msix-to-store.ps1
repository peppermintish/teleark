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
    Import-Module (Join-Path $PSScriptRoot 'msstore-submission-api.psm1') -Force
    $result = Invoke-MsStoreSubmission `
        -TenantId $env:AZURE_AD_TENANT_ID `
        -ClientId $env:AZURE_AD_APPLICATION_CLIENT_ID `
        -ProductId $env:TELEARK_MSSTORE_PRODUCT_ID `
        -PackagePath $PackagePath

    Write-Output "Microsoft Store submission committed; current status is $($result.Status)."
} finally {
    [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $null, 'Process')
}
