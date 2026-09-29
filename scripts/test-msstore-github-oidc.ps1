# Synthetic OIDC responses and a fake CLI; no live Store or GitHub credentials are used.
$ErrorActionPreference = 'Stop'
$modulePath = Join-Path $PSScriptRoot 'msstore-github-oidc.psm1'
$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testDirectory = [System.IO.Path]::GetFullPath(
    (Join-Path $tempRoot "teleark-msstore-oidc-$([guid]::NewGuid().ToString('N'))")
)
$allowedPrefix = Join-Path $tempRoot 'teleark-msstore-oidc-'
$environmentNames = @(
    'MSSTORE_CLIENT_ASSERTION',
    'ACTIONS_ID_TOKEN_REQUEST_URL',
    'ACTIONS_ID_TOKEN_REQUEST_TOKEN',
    'MSSTORE_TEST_CAPTURE_PATH',
    'MSSTORE_TEST_EXIT_CODE'
)
$previousEnvironment = @{}
foreach ($name in $environmentNames) {
    $previousEnvironment[$name] = [System.Environment]::GetEnvironmentVariable($name, 'Process')
}
$previousLastExitCode = $global:LASTEXITCODE

try {
    Import-Module $modulePath -Force
    New-Item -ItemType Directory -Path $testDirectory | Out-Null

    $tokenSentinel = 'synthetic-oidc-assertion-sentinel'
    $requestTokenSentinel = 'synthetic-github-request-token-sentinel'
    $requestCapture = [pscustomobject]@{ Uri = ''; Authorization = '' }
    $httpGet = {
        param($RequestUri, $Headers)
        $requestCapture.Uri = $RequestUri.AbsoluteUri
        $requestCapture.Authorization = $Headers.Authorization
        return @{ value = $tokenSentinel }
    }.GetNewClosure()
    $assertion = Get-MsStoreGitHubOidcToken `
        -RequestUrl 'https://token.example.test/request?api-version=2' `
        -RequestToken $requestTokenSentinel `
        -HttpGet $httpGet

    if ($assertion -ne $tokenSentinel) { throw 'OIDC token request returned an unexpected value.' }
    $decodedQuery = [uri]::UnescapeDataString(([uri]$requestCapture.Uri).Query)
    if ($decodedQuery -notmatch '(?:^|\?)api-version=2(?:&|$)' -or
        $decodedQuery -notmatch '(?:^|&)audience=api://AzureADTokenExchange(?:&|$)') {
        throw 'OIDC request did not preserve the request query and add the Store audience.'
    }
    if ($requestCapture.Authorization -ne "Bearer $requestTokenSentinel" -or
        $requestCapture.Uri.Contains($requestTokenSentinel) -or $requestCapture.Uri.Contains($tokenSentinel)) {
        throw 'OIDC request did not keep its bearer token in the authorization header.'
    }

    $emptyResponse = { return @{ value = '' } }
    $emptyTokenRejected = $false
    try {
        Get-MsStoreGitHubOidcToken `
            -RequestUrl 'https://token.example.test/request' `
            -RequestToken $requestTokenSentinel `
            -HttpGet $emptyResponse | Out-Null
    } catch {
        $emptyTokenRejected = $true
    }
    if (-not $emptyTokenRejected) { throw 'An empty OIDC response was accepted.' }

    $failureRequestUrlSentinel = 'https://token.example.test/oidc-request-endpoint-sentinel'
    $failureRequestTokenSentinel = 'synthetic-oidc-request-token-in-error-sentinel'
    $failedHttpGet = {
        param($RequestUri, $Headers)
        throw "Synthetic transport failure at $($RequestUri.AbsoluteUri) with $($Headers.Authorization)."
    }
    $transportFailureRedacted = $false
    try {
        Get-MsStoreGitHubOidcToken `
            -RequestUrl $failureRequestUrlSentinel `
            -RequestToken $failureRequestTokenSentinel `
            -HttpGet $failedHttpGet | Out-Null
    } catch {
        $errorRecordText = $_ | Out-String
        $transportFailureRedacted = $_.Exception.Message -eq 'GitHub Actions OIDC token request failed.' -and
            -not $errorRecordText.Contains($failureRequestUrlSentinel) -and
            -not $errorRecordText.Contains($failureRequestTokenSentinel)
    }
    if (-not $transportFailureRedacted) { throw 'An OIDC transport failure exposed request details.' }

    $fakeCliPath = Join-Path $testDirectory 'fake-msstore.ps1'
    $capturePath = Join-Path $testDirectory 'cli-observation.json'
    $fakeCli = @'
$observation = [pscustomobject]@{
    Arguments = @($args)
    Assertion = [System.Environment]::GetEnvironmentVariable('MSSTORE_CLIENT_ASSERTION', 'Process')
    RequestToken = [System.Environment]::GetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_TOKEN', 'Process')
}
[System.IO.File]::WriteAllText($env:MSSTORE_TEST_CAPTURE_PATH, ($observation | ConvertTo-Json -Compress))
$global:LASTEXITCODE = [int]$env:MSSTORE_TEST_EXIT_CODE
'@
    [System.IO.File]::WriteAllText($fakeCliPath, $fakeCli, [System.Text.UTF8Encoding]::new($false))

    $arguments = @(
        'reconfigure',
        '--tenantId',
        'synthetic-tenant-id',
        '--sellerId',
        'synthetic-seller-id',
        '--clientId',
        'synthetic-client-id',
        '--clientAssertion'
    )
    [System.Environment]::SetEnvironmentVariable('MSSTORE_CLIENT_ASSERTION', 'previous-assertion-sentinel', 'Process')
    [System.Environment]::SetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_URL', 'https://token.example.test/request', 'Process')
    [System.Environment]::SetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_TOKEN', $requestTokenSentinel, 'Process')
    [System.Environment]::SetEnvironmentVariable('MSSTORE_TEST_CAPTURE_PATH', $capturePath, 'Process')
    [System.Environment]::SetEnvironmentVariable('MSSTORE_TEST_EXIT_CODE', '0', 'Process')
    $tokenProvider = { return $tokenSentinel }.GetNewClosure()

    Invoke-MsStoreWithGitHubOidc -ArgumentList $arguments -CliCommand $fakeCliPath -TokenProvider $tokenProvider

    $observation = [System.IO.File]::ReadAllText($capturePath) | ConvertFrom-Json
    if ($observation.Assertion -ne $tokenSentinel -or $observation.RequestToken) {
        throw 'The CLI did not receive a scoped assertion without the GitHub request token.'
    }
    if ((Compare-Object -ReferenceObject $arguments -DifferenceObject @($observation.Arguments)) -or
        (@($observation.Arguments) -contains $tokenSentinel) -or
        (@($observation.Arguments) -contains $requestTokenSentinel) -or
        (@($observation.Arguments) -contains '--clientSecret')) {
        throw 'The CLI argument list included an unexpected value or credential.'
    }
    if ([System.Environment]::GetEnvironmentVariable('MSSTORE_CLIENT_ASSERTION', 'Process') -ne 'previous-assertion-sentinel' -or
        [System.Environment]::GetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_TOKEN', 'Process') -or
        [System.Environment]::GetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_URL', 'Process')) {
        throw 'OIDC environment values were not cleared or restored after the CLI call.'
    }

    [System.Environment]::SetEnvironmentVariable('MSSTORE_TEST_EXIT_CODE', '17', 'Process')
    $failureWasRedacted = $false
    try {
        Invoke-MsStoreWithGitHubOidc -ArgumentList $arguments -CliCommand $fakeCliPath -TokenProvider $tokenProvider
    } catch {
        $failureWasRedacted = $_.Exception.Message -match 'exit code 17' -and
            -not $_.Exception.Message.Contains($tokenSentinel) -and
            -not $_.Exception.Message.Contains($requestTokenSentinel)
    }
    if (-not $failureWasRedacted) { throw 'A CLI failure was not reported with a redacted message.' }
    if ([System.Environment]::GetEnvironmentVariable('MSSTORE_CLIENT_ASSERTION', 'Process') -ne 'previous-assertion-sentinel') {
        throw 'The OIDC assertion remained set after a failed CLI call.'
    }

    $workflowPath = Join-Path $PSScriptRoot '../.github/workflows/ci.yml'
    $workflow = [System.IO.File]::ReadAllText([System.IO.Path]::GetFullPath($workflowPath))
    $storeStart = $workflow.IndexOf("  store_submission:", [System.StringComparison]::Ordinal)
    if ($storeStart -lt 0) { throw 'The Store submission job was not found in the workflow.' }
    $publishStart = $workflow.IndexOf("  publish:", $storeStart, [System.StringComparison]::Ordinal)
    if ($publishStart -lt 0) { throw 'The GitHub Release job was not found after Store submission.' }
    $storeJob = $workflow.Substring($storeStart, $publishStart - $storeStart)
    if ($storeJob -notmatch '(?m)^    environment: store-submission$' -or
        $storeJob -notmatch '(?m)^      id-token: write$' -or
        $storeJob -notmatch '(?m)^          version: v0\.4\.2$' -or
        $storeJob -notmatch '--clientAssertion' -or
        [regex]::Matches($storeJob, 'invoke-msstore-with-github-oidc\.ps1').Count -ne 2 -or
        $storeJob -match '--clientSecret|AZURE_AD_APPLICATION_SECRET') {
        throw 'The Store submission workflow is not wired to the supported OIDC path.'
    }
} finally {
    Remove-Module -Name msstore-github-oidc -ErrorAction SilentlyContinue
    foreach ($name in $environmentNames) {
        [System.Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process')
    }
    $global:LASTEXITCODE = $previousLastExitCode

    if (-not $testDirectory.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a test directory outside the designated temporary directory.'
    }
    if (Test-Path -LiteralPath $testDirectory) { Remove-Item -LiteralPath $testDirectory -Recurse -Force }
}

Write-Output 'Microsoft Store GitHub OIDC authentication passed synthetic token, argv, environment, and failure-redaction cases.'
