param(
    [Parameter(Mandatory)]
    [ValidateNotNullOrEmpty()]
    [string[]]$ArgumentList
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'msstore-github-oidc.psm1') -Force
Invoke-MsStoreWithGitHubOidc -ArgumentList $ArgumentList
