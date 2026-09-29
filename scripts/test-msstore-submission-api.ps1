# Mocked Microsoft Store API requests; no live credentials or Store calls are used.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$modulePath = Join-Path $PSScriptRoot 'msstore-submission-api.psm1'
Import-Module $modulePath -Force
$tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testDirectory = [System.IO.Path]::GetFullPath(
    (Join-Path $tempRoot "teleark-store-api-test-$([guid]::NewGuid().ToString('N'))")
)
$packagePath = Join-Path $testDirectory 'TeleArk-1.2.3-windows-x86_64.msix'
$secretSentinel = 'synthetic-client-secret+/%& value'
$tokenSentinel = 'synthetic-access-token-never-log'
$sasSentinel = 'synthetic-sas-signature-never-log'
$requestSentinel = 'synthetic-request-error-never-log'
$tempArchivesBefore = @(Get-ChildItem -LiteralPath $tempRoot -Filter 'teleark-store-submission-*.zip' -ErrorAction SilentlyContinue)
$requests = [System.Collections.Generic.List[object]]::new()
$observations = @{
    UpdatedBody = $null
    ZipEntry = $null
    BlockList = $null
    StatusReads = 0
    Blocks = [System.Collections.Generic.List[object]]::new()
    ArchivePresentAtCreate = $false
}

function Assert-StoreApiTest {
    param([Parameter(Mandatory)][bool]$Condition, [Parameter(Mandatory)][string]$Message)
    if (-not $Condition) { throw $Message }
}

function Assert-ContainsExactlyOne {
    param([Parameter(Mandatory)][string]$Value, [Parameter(Mandatory)][string]$Container)
    $matches = [regex]::Matches($Container, [regex]::Escape($Value))
    Assert-StoreApiTest ($matches.Count -eq 1) 'A synthetic credential was not confined to its expected request field.'
}

$transport = {
    param($request)
    $requests.Add($request)

    if ($request.Uri -like 'https://login.microsoftonline.com/*/oauth2/token') {
        return @{ StatusCode = 200; Content = (@{ access_token = $tokenSentinel } | ConvertTo-Json -Compress) }
    }

    if ($request.Method -eq 'POST' -and $request.Uri -ceq 'https://manage.devcenter.microsoft.com/v1.0/my/applications/9NBLGGH4R315/submissions') {
        $observations.ArchivePresentAtCreate = @(
            Get-ChildItem -LiteralPath $tempRoot -Filter 'teleark-store-submission-*.zip' -ErrorAction SilentlyContinue
        ).Count -gt $tempArchivesBefore.Count
        $draft = [ordered]@{
            id = 'submission-123'
            status = 'PendingCommit'
            statusDetails = @{ errors = @(); warnings = @() }
            fileUploadUrl = "https://productingestionbin1.blob.core.windows.net/upload?sv=synthetic&sig=$sasSentinel"
            friendlyName = 'Submission 8'
            applicationCategory = 'Utilities_Utilities'
            pricing = @{
                priceId = 'Free'
                trialPeriod = 'NotAvailable'
                marketSpecificPricings = @{}
                sales = @()
                isAdvancedPricingModel = $true
            }
            visibility = 'Public'
            targetPublishMode = 'Manual'
            targetPublishDate = '1601-01-01T00:00:00Z'
            listings = @{ 'en-us' = @{ baseListing = @{ title = 'TeleArk'; description = 'Synthetic listing'; images = @() } } }
            hardwarePreferences = @('Keyboard', 'Mouse')
            automaticBackupEnabled = $false
            canInstallOnRemovableMedia = $true
            isGameDvrEnabled = $false
            gamingOptions = @()
            hasExternalInAppProducts = $false
            meetAccessibilityGuidelines = $true
            notesForCertification = ''
            applicationPackages = @(@{ fileName = 'prior.msix'; fileStatus = 'Uploaded' })
            packageDeliveryOptions = @{ packageRollout = @{ isPackageRollout = $false; packageRolloutPercentage = 0 }; isMandatoryUpdate = $false }
            enterpriseLicensing = 'None'
            allowMicrosoftDecideAppAvailabilityToFutureDeviceFamilies = $true
            allowTargetFutureDeviceFamilies = @{ Desktop = $true }
            trailers = @()
        }
        return @{ StatusCode = 201; Content = ($draft | ConvertTo-Json -Depth 100 -Compress) }
    }

    if ($request.Method -eq 'PUT' -and $request.Uri -ceq 'https://manage.devcenter.microsoft.com/v1.0/my/applications/9NBLGGH4R315/submissions/submission-123') {
        $observations.UpdatedBody = ConvertFrom-Json -InputObject $request.Body -AsHashtable -Depth 100
        return @{ StatusCode = 200; Content = '{}' }
    }

    if ($request.Method -eq 'PUT' -and $request.Uri -like '*comp=block&blockid=*') {
        Assert-StoreApiTest (-not $request.Headers.ContainsKey('Authorization')) 'The SAS package upload unexpectedly received an OAuth authorization header.'
        Assert-StoreApiTest ($request.Headers['x-ms-version'] -eq '2023-11-03') 'The Azure Blob Storage REST version was not set.'
        Assert-StoreApiTest ($request.Bytes.Length -gt 0) 'The package archive block was empty.'
        $observations.Blocks.Add(@{ Uri = [string]$request.Uri; Bytes = [byte[]]$request.Bytes })
        return @{ StatusCode = 201; Content = '' }
    }

    if ($request.Method -eq 'PUT' -and $request.Uri -like '*comp=blocklist*') {
        Assert-StoreApiTest ($request.Headers['x-ms-version'] -eq '2023-11-03') 'The Store package block list omitted the Azure REST version.'
        $observations.BlockList = [string]$request.Body
        $expectedIds = @('teleark-00000000', 'teleark-00000001') | ForEach-Object {
            [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($_))
        }
        Assert-StoreApiTest ($observations.Blocks.Count -eq 2) 'The multi-block Store upload did not produce exactly two ordered blocks.'
        for ($blockIndex = 0; $blockIndex -lt $observations.Blocks.Count; $blockIndex++) {
            Assert-StoreApiTest ($observations.Blocks[$blockIndex].Uri.Contains([Uri]::EscapeDataString($expectedIds[$blockIndex]))) 'The block upload order did not match its deterministic block IDs.'
        }
        $zipBuffer = [System.IO.MemoryStream]::new()
        foreach ($block in $observations.Blocks) { $zipBuffer.Write($block.Bytes, 0, $block.Bytes.Length) }
        $zipBuffer.Position = 0
        $zip = [System.IO.Compression.ZipArchive]::new($zipBuffer, [System.IO.Compression.ZipArchiveMode]::Read, $true)
        try {
            Assert-StoreApiTest ($zip.Entries.Count -eq 1) 'The Store package ZIP did not contain exactly one file.'
            Assert-StoreApiTest ($zip.Entries[0].FullName -ceq 'TeleArk-1.2.3-windows-x86_64.msix') 'The Store package ZIP entry did not match the MSIX filename.'
            $entryStream = $zip.Entries[0].Open()
            $entryBuffer = [System.IO.MemoryStream]::new()
            $entryStream.CopyTo($entryBuffer)
            $entryStream.Dispose()
            $observations.ZipEntry = [System.Text.Encoding]::UTF8.GetString($entryBuffer.ToArray())
            $entryBuffer.Dispose()
        } finally {
            $zip.Dispose()
            $zipBuffer.Dispose()
        }
        Assert-StoreApiTest ($observations.BlockList -ceq ("<?xml version=`"1.0`" encoding=`"utf-8`"?><BlockList><Latest>$($expectedIds[0])</Latest><Latest>$($expectedIds[1])</Latest></BlockList>")) 'The Azure block list did not preserve upload ordering.'
        return @{ StatusCode = 201; Content = '' }
    }

    if ($request.Method -eq 'POST' -and $request.Uri -ceq 'https://manage.devcenter.microsoft.com/v1.0/my/applications/9NBLGGH4R315/submissions/submission-123/commit') {
        return @{ StatusCode = 200; Content = '{"status":"CommitStarted"}' }
    }

    if ($request.Method -eq 'GET' -and $request.Uri -ceq 'https://manage.devcenter.microsoft.com/v1.0/my/applications/9NBLGGH4R315/submissions/submission-123/status') {
        $observations.StatusReads++
        $status = if ($observations.StatusReads -eq 1) { 'CommitStarted' } else { 'PreProcessing' }
        return @{ StatusCode = 200; Content = (@{ status = $status } | ConvertTo-Json -Compress) }
    }

    throw [System.InvalidOperationException]::new('Unexpected synthetic HTTP request.')
}.GetNewClosure()

try {
    New-Item -ItemType Directory -Path $testDirectory | Out-Null
    $syntheticPackageBytes = [byte[]]::new(16MB + 17)
    [Array]::Fill[byte]($syntheticPackageBytes, [byte][char]'S')
    $syntheticPackageBytes[0] = [byte][char]'T'
    [System.IO.File]::WriteAllBytes($packagePath, $syntheticPackageBytes)
    $sleepCalls = [System.Collections.Generic.List[int]]::new()
    $sleep = { param($seconds) $sleepCalls.Add([int]$seconds) }.GetNewClosure()
    [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $secretSentinel, 'Process')

    $result = Invoke-MsStoreSubmission `
        -TenantId 'tenant-123' `
        -ClientId 'client-456' `
        -ProductId '9NBLGGH4R315' `
        -PackagePath $packagePath `
        -HttpTransport $transport `
        -SleepAction $sleep `
        -PollTimeoutSeconds 10 `
        -PollIntervalSeconds 0

    Assert-StoreApiTest ($result.Status -eq 'PreProcessing') 'The synthetic Store submission did not report the committed state.'
    Assert-StoreApiTest ($requests.Count -eq 9) "The synthetic Store submission made $($requests.Count) requests instead of the expected sequence."
    Assert-StoreApiTest ($observations.ArchivePresentAtCreate) 'The package ZIP was not prepared before creating the Store draft.'
    Assert-StoreApiTest ([string]::IsNullOrEmpty([System.Environment]::GetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', 'Process'))) 'The client secret remained in the process environment after token acquisition.'
    Assert-StoreApiTest ($requests[0].Uri -eq 'https://login.microsoftonline.com/tenant-123/oauth2/token') 'The Microsoft identity token endpoint was incorrect.'
    Assert-StoreApiTest ($requests[0].ContentType -eq 'application/x-www-form-urlencoded') 'The client-credentials request did not use a form body.'
    $encodedSecret = [System.Net.WebUtility]::UrlEncode($secretSentinel)
    Assert-ContainsExactlyOne -Value "client_secret=$encodedSecret" -Container $requests[0].Body
    Assert-StoreApiTest (-not $requests[0].Headers.ContainsKey('Authorization')) 'The token request unexpectedly sent a bearer header.'

    $apiRequests = @($requests | Where-Object { $_.Uri -like 'https://manage.devcenter.microsoft.com/*' })
    Assert-StoreApiTest ($apiRequests.Count -eq 5) "The mocked Store API produced $($apiRequests.Count) authorized Store requests instead of the expected sequence."
    foreach ($request in $apiRequests) {
        Assert-StoreApiTest ($request.Headers['Authorization'] -eq "Bearer $tokenSentinel") 'A Store API request omitted its bearer token.'
        Assert-StoreApiTest (-not ([string]$request.Uri).Contains($tokenSentinel)) 'The bearer token appeared in a Store API URL.'
    }

    $updateRequest = @($requests | Where-Object { $_.Method -eq 'PUT' -and $_.Uri -like 'https://manage.devcenter.microsoft.com/*/submissions/submission-123' })[0]
    Assert-StoreApiTest ($observations.UpdatedBody.pricing.priceId -eq 'Free') 'The update did not preserve the published pricing settings.'
    Assert-StoreApiTest ($observations.UpdatedBody.pricing.trialPeriod -eq 'NotAvailable') 'The update did not preserve the trial period.'
    Assert-StoreApiTest (-not $observations.UpdatedBody.pricing.Contains('isAdvancedPricingModel')) 'A read-only pricing field was included in the update.'
    Assert-StoreApiTest (-not $observations.UpdatedBody.pricing.Contains('sales')) 'A deprecated, read-only pricing field was included in the update.'
    Assert-StoreApiTest ($observations.UpdatedBody.listings.'en-us'.baseListing.title -eq 'TeleArk') 'The update did not preserve the Store listing.'
    Assert-StoreApiTest ($observations.UpdatedBody.applicationPackages.Count -eq 1) 'The update did not replace the submission package set with the verified MSIX.'
    $packageResource = $observations.UpdatedBody.applicationPackages[0]
    Assert-StoreApiTest ($packageResource.fileName -ceq 'TeleArk-1.2.3-windows-x86_64.msix') 'The application package filename was incorrect.'
    Assert-StoreApiTest ($packageResource.fileStatus -ceq 'PendingUpload') 'The package was not marked PendingUpload.'
    Assert-StoreApiTest ($packageResource.minimumDirectXVersion -ceq 'None' -and $packageResource.minimumSystemRam -ceq 'None') 'The required package defaults were not preserved.'
    foreach ($readOnlyField in @('id', 'status', 'statusDetails', 'fileUploadUrl', 'friendlyName')) {
        Assert-StoreApiTest (-not $observations.UpdatedBody.Contains($readOnlyField)) 'A response-only field was included in the submission update.'
    }
    Assert-StoreApiTest ($null -ne $updateRequest) 'The submission update request was missing.'
    Assert-StoreApiTest ($observations.ZipEntry.Length -eq $syntheticPackageBytes.Length) 'The Store upload ZIP did not contain the original MSIX bytes.'
    Assert-StoreApiTest ($observations.ZipEntry[0] -eq 'T' -and $observations.ZipEntry[-1] -eq 'S') 'The Store upload ZIP did not preserve the MSIX contents.'
    Assert-StoreApiTest ($observations.BlockList -match '<Latest>[A-Za-z0-9+/=]+</Latest>') 'The Azure block upload was not committed with a block list.'
    Assert-StoreApiTest ($observations.StatusReads -eq 2) 'The submission status was not polled until PreProcessing.'
    $tempArchivesAfter = @(Get-ChildItem -LiteralPath $tempRoot -Filter 'teleark-store-submission-*.zip' -ErrorAction SilentlyContinue)
    Assert-StoreApiTest ($tempArchivesAfter.Count -eq $tempArchivesBefore.Count) 'The temporary package ZIP was not removed.'

    $loggedOutput = [string]$result
    foreach ($sensitiveSentinel in @($secretSentinel, $tokenSentinel, $sasSentinel)) {
        Assert-StoreApiTest (-not $loggedOutput.Contains($sensitiveSentinel)) 'A synthetic credential or SAS value appeared in command output.'
    }
    foreach ($request in $requests) {
        if ($request.Method -ne 'POST' -or $request.Uri -notlike 'https://login.microsoftonline.com/*') {
            $requestBody = if ($request.Contains('Body')) { [string]$request.Body } else { '' }
            $serializedRequest = [string]::Join(' ', @($request.Uri, ($request.Headers | ConvertTo-Json -Compress), $requestBody))
            Assert-StoreApiTest (-not $serializedRequest.Contains($secretSentinel)) 'The client secret escaped the OAuth form body.'
        }
    }

    [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $secretSentinel, 'Process')
    $failureTransport = {
        param($request)
        throw [System.InvalidOperationException]::new("$secretSentinel $tokenSentinel $sasSentinel $requestSentinel")
    }.GetNewClosure()
    try {
        Invoke-MsStoreSubmission -TenantId 'tenant-123' -ClientId 'client-456' `
            -ProductId '9NBLGGH4R315' -PackagePath $packagePath -HttpTransport $failureTransport
        throw 'A synthetic transport failure unexpectedly succeeded.'
    } catch {
        $failure = $_.Exception.Message
        Assert-StoreApiTest ($failure -eq 'The Store submission request failed during TokenRequest. Inspect Partner Center before retrying.') 'A token failure was not safely redacted.'
        foreach ($sensitiveSentinel in @($secretSentinel, $tokenSentinel, $sasSentinel, $requestSentinel)) {
            Assert-StoreApiTest (-not $failure.Contains($sensitiveSentinel)) 'A failed request exposed a synthetic credential or request URL.'
        }
    }

    $creationFailureRequests = [System.Collections.Generic.List[object]]::new()
    [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $secretSentinel, 'Process')
    $creationFailureTransport = {
        param($request)
        $creationFailureRequests.Add($request)
        if ($request.Uri -like 'https://login.microsoftonline.com/*') {
            return @{ StatusCode = 200; Content = (@{ access_token = $tokenSentinel } | ConvertTo-Json -Compress) }
        }
        throw [System.InvalidOperationException]::new("$secretSentinel $tokenSentinel $sasSentinel $requestSentinel")
    }.GetNewClosure()
    try {
        Invoke-MsStoreSubmission -TenantId 'tenant-123' -ClientId 'client-456' `
            -ProductId '9NBLGGH4R315' -PackagePath $packagePath -HttpTransport $creationFailureTransport
        throw 'An ambiguous synthetic draft-creation failure unexpectedly succeeded.'
    } catch {
        $creationFailure = $_.Exception.Message
        Assert-StoreApiTest ($creationFailure -match 'creation request failed or timed out.*may have created a draft.*inspect Partner Center') 'An ambiguous draft creation did not provide safe recovery guidance.'
        foreach ($sensitiveSentinel in @($secretSentinel, $tokenSentinel, $sasSentinel, $requestSentinel)) {
            Assert-StoreApiTest (-not $creationFailure.Contains($sensitiveSentinel)) 'An ambiguous draft-creation failure exposed a synthetic credential or SAS URL.'
        }
        $createAttempts = @($creationFailureRequests | Where-Object { $_.Method -eq 'POST' -and $_.Uri -like 'https://manage.devcenter.microsoft.com/*/submissions' })
        Assert-StoreApiTest ($createAttempts.Count -eq 1) 'An ambiguous submission creation failure was retried automatically.'
    }

    $conflictRequests = [System.Collections.Generic.List[object]]::new()
    [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $secretSentinel, 'Process')
    $conflictTransport = {
        param($request)
        $conflictRequests.Add($request)
        if ($request.Uri -like 'https://login.microsoftonline.com/*') {
            return @{ StatusCode = 200; Content = (@{ access_token = $tokenSentinel } | ConvertTo-Json -Compress) }
        }
        return @{ StatusCode = 409; Content = "synthetic-conflict $secretSentinel $sasSentinel" }
    }.GetNewClosure()
    try {
        Invoke-MsStoreSubmission -TenantId 'tenant-123' -ClientId 'client-456' `
            -ProductId '9NBLGGH4R315' -PackagePath $packagePath -HttpTransport $conflictTransport
        throw 'A synthetic active-draft conflict unexpectedly succeeded.'
    } catch {
        $conflict = $_.Exception.Message
        Assert-StoreApiTest ($conflict -match 'HTTP 409.*existing draft.*unsupported app setting') 'A Store 409 did not provide actionable draft guidance.'
        foreach ($sensitiveSentinel in @($secretSentinel, $tokenSentinel, $sasSentinel)) {
            Assert-StoreApiTest (-not $conflict.Contains($sensitiveSentinel)) 'A 409 response exposed a synthetic credential or SAS URL.'
        }
        $createAttempts = @($conflictRequests | Where-Object { $_.Method -eq 'POST' -and $_.Uri -like 'https://manage.devcenter.microsoft.com/*/submissions' })
        Assert-StoreApiTest ($createAttempts.Count -eq 1) 'A failed submission creation request was retried automatically.'
    }

    $uploadFailureRequests = [System.Collections.Generic.List[object]]::new()
    [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $secretSentinel, 'Process')
    $uploadFailureTransport = {
        param($request)
        $uploadFailureRequests.Add($request)
        if ($request.Uri -like 'https://login.microsoftonline.com/*') {
            return @{ StatusCode = 200; Content = (@{ access_token = $tokenSentinel } | ConvertTo-Json -Compress) }
        }
        if ($request.Method -eq 'POST' -and $request.Uri -like 'https://manage.devcenter.microsoft.com/*/submissions') {
            return @{ StatusCode = 201; Content = (@{
                id = 'submission-123'
                fileUploadUrl = "https://productingestionbin1.blob.core.windows.net/upload?sig=$sasSentinel"
                pricing = @{ priceId = 'Free'; trialPeriod = 'NotAvailable'; isAdvancedPricingModel = $true }
            } | ConvertTo-Json -Depth 20 -Compress) }
        }
        if ($request.Method -eq 'PUT' -and $request.Uri -like 'https://manage.devcenter.microsoft.com/*/submissions/submission-123') {
            return @{ StatusCode = 200; Content = '{}' }
        }
        if ($request.Method -eq 'PUT' -and $request.Uri -like '*comp=block&blockid=*') {
            Assert-StoreApiTest (-not $request.Headers.ContainsKey('Authorization')) 'The failed SAS upload unexpectedly received an OAuth authorization header.'
            return @{ StatusCode = 503; Content = "$secretSentinel $tokenSentinel $sasSentinel $requestSentinel" }
        }
        throw 'Unexpected synthetic upload-failure request.'
    }.GetNewClosure()
    try {
        Invoke-MsStoreSubmission -TenantId 'tenant-123' -ClientId 'client-456' `
            -ProductId '9NBLGGH4R315' -PackagePath $packagePath -HttpTransport $uploadFailureTransport
        throw 'A synthetic Store package upload failure unexpectedly succeeded.'
    } catch {
        $uploadFailure = $_.Exception.Message
        Assert-StoreApiTest ($uploadFailure -match 'package upload failed \(HTTP 503\).*in-progress draft may remain') 'An upload failure did not preserve its safe HTTP status and draft guidance.'
        foreach ($sensitiveSentinel in @($secretSentinel, $tokenSentinel, $sasSentinel, $requestSentinel)) {
            Assert-StoreApiTest (-not $uploadFailure.Contains($sensitiveSentinel)) 'An upload failure exposed a synthetic credential or SAS URL.'
        }
        $uploadAttempts = @($uploadFailureRequests | Where-Object { $_.Method -eq 'PUT' -and $_.Uri -like '*comp=block&blockid=*' })
        Assert-StoreApiTest ($uploadAttempts.Count -eq 1) 'A failed package upload block was retried automatically.'
    }

    $workflowPath = Join-Path $PSScriptRoot '..\.github\workflows\ci.yml'
    $workflow = [System.IO.File]::ReadAllText([System.IO.Path]::GetFullPath($workflowPath))
    Assert-StoreApiTest ($workflow -notmatch 'id-token:\s*write|MSSTORE_CLIENT_ASSERTION|github-oidc|invoke-msstore-with-github-oidc') 'The workflow retained an operational OIDC dependency.'
    Assert-StoreApiTest ($workflow -match 'AZURE_AD_APPLICATION_SECRET:\s*\$\{\{\s*secrets\.AZURE_AD_APPLICATION_SECRET\s*\}\}') 'The workflow did not expose the existing Store secret through an environment variable.'
    Assert-StoreApiTest ($workflow -match 'submit-msix-to-store\.ps1\s+-PackagePath') 'The Store script is not called with a file path only.'
    Assert-StoreApiTest ($workflow -notmatch '\$\{\{\s*secrets\.AZURE_AD_APPLICATION_SECRET\s*\}\}[^\r\n]*run:|run:[^\r\n]*\$\{\{\s*secrets\.AZURE_AD_APPLICATION_SECRET') 'The client secret was interpolated into a command argument.'
    $submissionCommand = Get-Command -Name Invoke-MsStoreSubmission -Module msstore-submission-api
    Assert-StoreApiTest (-not $submissionCommand.Parameters.ContainsKey('ClientSecret')) 'The submission helper exposed a client secret argument.'
} finally {
    [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $null, 'Process')
    Remove-Module -Name msstore-submission-api -ErrorAction SilentlyContinue
    $allowedPrefix = Join-Path $tempRoot 'teleark-store-api-test-'
    if (-not $testDirectory.StartsWith($allowedPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a synthetic test directory outside the temporary directory.'
    }
    if (Test-Path -LiteralPath $testDirectory) { Remove-Item -LiteralPath $testDirectory -Recurse -Force }
}

Write-Output 'Microsoft Store Submission API passed mocked token, metadata, ZIP upload, commit, polling, and redaction cases.'
