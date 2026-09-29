$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workflowPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\.github\workflows\ci.yml'))
$packageWorkflowPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\.github\workflows\package-platform.yml'))
$workflow = [System.IO.File]::ReadAllText($workflowPath)
$packageWorkflow = [System.IO.File]::ReadAllText($packageWorkflowPath)
$storeWorkflowPath = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\.github\workflows\store-submission.yml'))
$storeWorkflow = [System.IO.File]::ReadAllText($storeWorkflowPath)

function Get-CiWorkflowJob {
    param([Parameter(Mandatory)][string]$Source, [Parameter(Mandatory)][string]$Name)

    $escapedName = [System.Text.RegularExpressions.Regex]::Escape($Name)
    $pattern = "(?ms)^  ${escapedName}:`r?`n(?<body>.*?)(?=^  [A-Za-z0-9_]+:`r?`n|\z)"
    $match = [System.Text.RegularExpressions.Regex]::Match($Source, $pattern)
    if (-not $match.Success) { throw "CI workflow job '$Name' was not found." }
    return $match.Groups['body'].Value
}

function Assert-CiWorkflow {
    param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
    if (-not $Condition) { throw $Message }
}

$preflight = Get-CiWorkflowJob -Source $workflow -Name 'preflight'
$quality = Get-CiWorkflowJob -Source $workflow -Name 'quality'
$tests = Get-CiWorkflowJob -Source $workflow -Name 'test'
$windowsPackage = Get-CiWorkflowJob -Source $workflow -Name 'package_windows'
$macosPackage = Get-CiWorkflowJob -Source $workflow -Name 'package_macos'
$linuxPackage = Get-CiWorkflowJob -Source $workflow -Name 'package_linux'
$store = Get-CiWorkflowJob -Source $workflow -Name 'store_submission'
$publish = Get-CiWorkflowJob -Source $workflow -Name 'publish'

Assert-CiWorkflow ($preflight -notmatch '(?m)^\s+needs:') 'Preflight must remain independent and lightweight.'
Assert-CiWorkflow ($preflight -match 'Verify release commit is on main' -and $preflight -match 'Validate Microsoft Store package identity') 'Preflight lost release tag or package identity validation.'
Assert-CiWorkflow ($preflight -match 'TELEARK_DISTRIBUTION_TELEGRAM_API_ID' -and $preflight -match 'TELEARK_MACOS_SIGNING_P12_BASE64') 'Preflight lost required distribution credential checks.'
Assert-CiWorkflow ($preflight -notmatch 'check-msstore-submit-config\.ps1|AZURE_AD_TENANT_ID|AZURE_AD_APPLICATION_CLIENT_ID|AZURE_AD_APPLICATION_SECRET|TELEARK_MSSTORE_PRODUCT_ID') 'Preflight must not read Store submission settings or credentials.'

foreach ($job in @(@{ Name = 'quality'; Body = $quality }, @{ Name = 'test'; Body = $tests }, @{ Name = 'package_windows'; Body = $windowsPackage }, @{ Name = 'package_macos'; Body = $macosPackage }, @{ Name = 'package_linux'; Body = $linuxPackage })) {
    Assert-CiWorkflow ($job.Body -match '(?m)^\s+needs: preflight$') "The $($job.Name) job must begin after preflight and run independently."
}
Assert-CiWorkflow ($tests -match 'Linux x64' -and $tests -match 'Windows x64' -and $tests -match 'macOS arm64') 'Linux, Windows, and macOS tests must run as separate matrix legs.'
Assert-CiWorkflow ($windowsPackage -match 'uses: \.\/\.github\/workflows\/package-platform\.yml' -and $macosPackage -match 'uses: \.\/\.github\/workflows\/package-platform\.yml' -and $linuxPackage -match 'uses: \.\/\.github\/workflows\/package-platform\.yml') 'Each platform must use the shared package workflow.'

Assert-CiWorkflow ($store -match '(?m)^\s+needs: publish$') 'Store submission must wait for the published, verified cross-platform release.'
$nonStoreJobs = @($quality, $tests, $windowsPackage, $macosPackage, $linuxPackage, $packageWorkflow) -join "`n"
Assert-CiWorkflow ($nonStoreJobs -notmatch 'AZURE_AD_TENANT_ID|AZURE_AD_APPLICATION_CLIENT_ID|AZURE_AD_APPLICATION_SECRET') 'Store authentication settings must remain scoped to the Store job.'
Assert-CiWorkflow ($store -match 'uses: \./\.github/workflows/store-submission\.yml') 'Tagged releases must call the independently retryable Store workflow.'
Assert-CiWorkflow ($storeWorkflow -match '(?m)^\s+environment: store-submission$') 'Store credentials must be scoped to the protected Store environment.'
$storeConfigCheck = [regex]::Match($storeWorkflow, '(?ms)- name: Validate Microsoft Store submission configuration(?<step>.*?)(?=^      - name:|\z)')
$storeSubmission = [regex]::Match($storeWorkflow, '(?m)^      - name: Upload MSIX ZIP and commit the Partner Center submission$')
$verifyStorePackage = [regex]::Match($storeWorkflow, '(?m)^      - name: Download and verify the published release MSIX$')
Assert-CiWorkflow ($storeConfigCheck.Success -and $storeConfigCheck.Groups['step'].Value -match 'check-msstore-submit-config\.ps1 -RequireSubmission') 'Store submission configuration must be validated inside the protected Store job.'
Assert-CiWorkflow ($storeConfigCheck.Groups['step'].Value -match 'AZURE_AD_TENANT_ID: \$\{\{ secrets\.AZURE_AD_TENANT_ID \}\}' -and $storeConfigCheck.Groups['step'].Value -match 'AZURE_AD_APPLICATION_CLIENT_ID: \$\{\{ secrets\.AZURE_AD_APPLICATION_CLIENT_ID \}\}' -and $storeConfigCheck.Groups['step'].Value -match 'AZURE_AD_APPLICATION_SECRET: \$\{\{ secrets\.AZURE_AD_APPLICATION_SECRET \}\}' -and $storeConfigCheck.Groups['step'].Value -match 'TELEARK_MSSTORE_PRODUCT_ID: \$\{\{ vars\.TELEARK_MSSTORE_PRODUCT_ID \}\}') 'Protected Store validation must check tenant, client, secret and Product ID settings.'
Assert-CiWorkflow ($verifyStorePackage.Success -and $verifyStorePackage.Index -lt $storeConfigCheck.Index -and $storeConfigCheck.Index -lt $storeSubmission.Index) 'Store configuration validation must follow package verification and precede API submission.'
Assert-CiWorkflow ($publish -match '(?m)^\s+needs: \[preflight, quality, test, package_windows, package_macos, package_linux\]$') 'GitHub Release must wait for every quality/test/package gate independently of Store availability.'
Assert-CiWorkflow ($store -match 'github\.event_name == ''push'' && startsWith\(github\.ref, ''refs/tags/v''\)' -and $publish -match 'github\.event_name == ''push'' && startsWith\(github\.ref, ''refs/tags/v''\)') 'Store submission and GitHub Release must run only for version-tag pushes.'
Assert-CiWorkflow ($workflow -notmatch 'id-token:\s*write|MSSTORE_CLIENT_ASSERTION|github-oidc|invoke-msstore-with-github-oidc|microsoft-store-apppublisher') 'Operational OIDC or Store CLI wiring remains in the workflow.'
Assert-CiWorkflow ($workflow -match 'AZURE_AD_APPLICATION_SECRET:\s*\$\{\{\s*secrets\.AZURE_AD_APPLICATION_SECRET\s*\}\}') 'The client secret must be supplied as a workflow environment value.'
Assert-CiWorkflow ($storeWorkflow -match '(?m)^  workflow_call:$' -and $storeWorkflow -match '(?m)^  workflow_dispatch:$') 'Store delivery must support automatic releases and manual retries.'
Assert-CiWorkflow ($storeWorkflow -match "github.ref == 'refs/heads/main'" -and $storeWorkflow -match 'github.ref_name == inputs.release_tag') 'Store dispatch must use trusted main code and automatic calls must match the release tag.'
Assert-CiWorkflow ($storeWorkflow -match 'group: teleark-microsoft-store' -and $storeWorkflow -match 'cancel-in-progress: false') 'Store mutations must be serialized across version tags and retries.'
Assert-CiWorkflow ($storeWorkflow -match 'msstore_submission\.py prepare' -and $storeWorkflow -match 'msstore_submission\.py submit') 'Store delivery must verify released bytes before submitting them.'
Assert-CiWorkflow ($storeWorkflow -match 'Retain sanitized submission receipt' -and $storeWorkflow -match 'if: \$\{\{ always\(\) \}\}') 'Store receipts must remain available on failure.'
Assert-CiWorkflow ($tests -match 'Test Store submission on the native runner without credentials') 'Native OS tests must exercise the portable Store client.'
Assert-CiWorkflow (($workflow + $storeWorkflow) -notmatch 'test-msstore-submission-api\.ps1|msstore-submission-api\.psm1') 'CI must not use the retired PowerShell HTTP client.'

Assert-CiWorkflow ($packageWorkflow -match '(?m)^  workflow_call:$') 'The platform package workflow must be reusable.'
Assert-CiWorkflow ($packageWorkflow -match 'release-\$\{\{ inputs\.artifact \}\}' -and $packageWorkflow -match 'actions/upload-artifact@') 'The reusable package workflow must publish a named, verified artifact.'
Assert-CiWorkflow ($packageWorkflow -match "cargo test[^\r\n]+--features msix data_root\r?\n\s+if \(\`$LASTEXITCODE -ne 0\)") 'A failing MSIX data-layout test must stop Windows packaging before compilation.'

Write-Output 'CI workflow graph passed release gates, independent Store retry, protected credentials, serialized submission, receipt, and preview-safety checks.'
