Set-StrictMode -Version Latest

$script:MsStoreSubmissionApiBaseUri = 'https://manage.devcenter.microsoft.com/v1.0/my/applications'
$script:AzureBlobApiVersion = '2023-11-03'
$script:MsStoreUploadBlockSize = 16MB
$script:MsStoreMaximumBlockCount = 50000
$script:MsStoreHttpClient = $null
$script:MsStoreHttpTransportDiagnosticKey = [object]::new()

function ConvertTo-MsStoreFormBody {
    param([Parameter(Mandatory)][System.Collections.IDictionary]$Values)

    $parts = foreach ($key in $Values.Keys) {
        '{0}={1}' -f [System.Net.WebUtility]::UrlEncode([string]$key),
            [System.Net.WebUtility]::UrlEncode([string]$Values[$key])
    }
    return [string]::Join('&', $parts)
}

function Get-MsStoreHttpClient {
    if ($null -eq $script:MsStoreHttpClient) {
        $handler = [System.Net.Http.HttpClientHandler]::new()
        $handler.AllowAutoRedirect = $false
        $script:MsStoreHttpClient = [System.Net.Http.HttpClient]::new($handler)
        $script:MsStoreHttpClient.Timeout = [TimeSpan]::FromMinutes(30)
    }
    return $script:MsStoreHttpClient
}

function Get-MsStoreHttpTransportDiagnostic {
    param(
        [Parameter(Mandatory)][System.Exception]$Exception,
        [Parameter(Mandatory)][ValidateSet('request-setup', 'send', 'response-read')][string]$Phase
    )

    $exceptionTypeNames = @{
        'System.Exception' = 'Exception'
        'System.IO.IOException' = 'IOException'
        'System.InvalidOperationException' = 'InvalidOperationException'
        'System.Net.Http.HttpRequestException' = 'HttpRequestException'
        'System.Net.Sockets.SocketException' = 'SocketException'
        'System.OperationCanceledException' = 'OperationCanceledException'
        'System.Security.Authentication.AuthenticationException' = 'AuthenticationException'
        'System.Threading.Tasks.TaskCanceledException' = 'TaskCanceledException'
        'System.TimeoutException' = 'TimeoutException'
    }

    $diagnosticException = $Exception
    $category = 'other'
    $current = $Exception
    while ($null -ne $current) {
        if ($current -is [System.TimeoutException] -or
            $current -is [System.Threading.Tasks.TaskCanceledException] -or
            $current -is [System.OperationCanceledException]) {
            $diagnosticException = $current
            $category = 'timeout'
            break
        }
        if ($current -is [System.Security.Authentication.AuthenticationException]) {
            $diagnosticException = $current
            $category = 'tls'
            break
        }
        if ($current -is [System.Net.Sockets.SocketException]) {
            $diagnosticException = $current
            $socketError = [string]$current.SocketErrorCode
            if ($socketError -in @('HostNotFound', 'TryAgain', 'NoData', 'NoRecovery')) {
                $category = 'dns'
            } elseif ($socketError -ceq 'TimedOut') {
                $category = 'timeout'
            } else {
                $category = 'socket'
            }
            break
        }
        $current = $current.InnerException
    }

    $typeName = [string]$diagnosticException.GetType().FullName
    if (-not $exceptionTypeNames.ContainsKey($typeName)) {
        $diagnosticException = $Exception
        $typeName = [string]$diagnosticException.GetType().FullName
    }
    $safeTypeName = if ($exceptionTypeNames.ContainsKey($typeName)) {
        $exceptionTypeNames[$typeName]
    } else {
        'Exception'
    }
    $hresultBytes = [BitConverter]::GetBytes([int]$diagnosticException.HResult)
    $hresult = '0x{0:X8}' -f [BitConverter]::ToUInt32($hresultBytes, 0)

    return @{
        Phase = $Phase
        Category = $category
        ExceptionType = $safeTypeName
        HResult = $hresult
    }
}

function New-MsStoreHttpTransportException {
    param(
        [Parameter(Mandatory)][System.Exception]$Exception,
        [Parameter(Mandatory)][ValidateSet('request-setup', 'send', 'response-read')][string]$Phase
    )

    $safeException = [System.InvalidOperationException]::new('HTTP transport request failed.')
    $safeException.Data[$script:MsStoreHttpTransportDiagnosticKey] =
        Get-MsStoreHttpTransportDiagnostic -Exception $Exception -Phase $Phase
    return $safeException
}

function Get-MsStoreHttpTransportDiagnosticSuffix {
    param([Parameter(Mandatory)][System.Exception]$Exception)

    if (-not $Exception.Data.Contains($script:MsStoreHttpTransportDiagnosticKey)) {
        return ''
    }

    $diagnostic = $Exception.Data[$script:MsStoreHttpTransportDiagnosticKey]
    $validPhases = @('request-setup', 'send', 'response-read')
    $validCategories = @('dns', 'other', 'socket', 'timeout', 'tls')
    $validExceptionTypes = @(
        'AuthenticationException', 'Exception', 'HttpRequestException', 'IOException',
        'InvalidOperationException', 'OperationCanceledException', 'SocketException',
        'TaskCanceledException', 'TimeoutException'
    )
    if ($diagnostic -isnot [System.Collections.IDictionary] -or
        [string]$diagnostic.Phase -cnotin $validPhases -or
        [string]$diagnostic.Category -cnotin $validCategories -or
        [string]$diagnostic.ExceptionType -cnotin $validExceptionTypes -or
        [string]$diagnostic.HResult -cnotmatch '^0x[0-9A-F]{8}$') {
        return ''
    }

    return " [transport phase=$($diagnostic.Phase); category=$($diagnostic.Category); exception=$($diagnostic.ExceptionType); HResult=$($diagnostic.HResult)]"
}

function Invoke-MsStoreHttpRequest {
    param([Parameter(Mandatory)][System.Collections.IDictionary]$Request)

    $client = Get-MsStoreHttpClient
    $message = $null
    $stream = $null
    $response = $null
    try {
        try {
            $message = [System.Net.Http.HttpRequestMessage]::new(
                [System.Net.Http.HttpMethod]::new([string]$Request.Method),
                [string]$Request.Uri
            )

            foreach ($header in $Request.Headers.GetEnumerator()) {
                if (-not $message.Headers.TryAddWithoutValidation([string]$header.Key, [string]$header.Value)) {
                    throw 'Request header could not be applied.'
                }
            }

            if ($Request.Contains('FilePath')) {
                $stream = [System.IO.File]::OpenRead([string]$Request.FilePath)
                $message.Content = [System.Net.Http.StreamContent]::new($stream)
                $message.Content.Headers.ContentType = [System.Net.Http.Headers.MediaTypeHeaderValue]::new(
                    [string]$Request.ContentType
                )
            } elseif ($Request.Contains('Bytes')) {
                $message.Content = [System.Net.Http.ByteArrayContent]::new([byte[]]$Request.Bytes)
                $message.Content.Headers.ContentType = [System.Net.Http.Headers.MediaTypeHeaderValue]::new(
                    [string]$Request.ContentType
                )
            } elseif ($Request.Contains('Body')) {
                $message.Content = [System.Net.Http.StringContent]::new(
                    [string]$Request.Body,
                    [System.Text.Encoding]::UTF8,
                    [string]$Request.ContentType
                )
            }
        } catch {
            throw (New-MsStoreHttpTransportException -Exception $_.Exception -Phase 'request-setup')
        }

        try {
            $response = $client.SendAsync($message).GetAwaiter().GetResult()
        } catch {
            throw (New-MsStoreHttpTransportException -Exception $_.Exception -Phase 'send')
        }
        try {
            $content = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        } catch {
            throw (New-MsStoreHttpTransportException -Exception $_.Exception -Phase 'response-read')
        }
        return @{
            StatusCode = [int]$response.StatusCode
            Content = $content
        }
    } catch {
        if ($_.Exception.Data.Contains($script:MsStoreHttpTransportDiagnosticKey)) { throw }
        throw [System.InvalidOperationException]::new('HTTP transport request failed.')
    } finally {
        if ($null -ne $response) { $response.Dispose() }
        if ($null -ne $message) { $message.Dispose() }
        if ($null -ne $stream) { $stream.Dispose() }
    }
}

function Invoke-MsStoreSafeRequest {
    param(
        [Parameter(Mandatory)][System.Collections.IDictionary]$Request,
        [Parameter(Mandatory)][scriptblock]$Transport,
        [Parameter(Mandatory)][string]$Operation
    )

    try {
        $response = & $Transport $Request
    } catch {
        $diagnosticSuffix = Get-MsStoreHttpTransportDiagnosticSuffix -Exception $_.Exception
        if ($Operation -eq 'CreateSubmission') {
            throw [System.InvalidOperationException]::new(
                "The Store submission creation request failed or timed out$diagnosticSuffix. It may have created a draft; inspect Partner Center before retrying."
            )
        }
        if ($Operation -eq 'UploadPackageBlock' -or $Operation -eq 'CommitPackageUpload') {
            throw [System.InvalidOperationException]::new(
                "The Store package upload request failed$diagnosticSuffix. An in-progress draft may remain; inspect Partner Center before retrying."
            )
        }
        throw [System.InvalidOperationException]::new(
            "The Store submission request failed during $Operation$diagnosticSuffix. Inspect Partner Center before retrying."
        )
    }

    if ($null -eq $response -or $null -eq $response.StatusCode) {
        throw [System.InvalidOperationException]::new("The Store submission response was invalid during $Operation.")
    }

    $statusCode = [int]$response.StatusCode
    if ($statusCode -lt 200 -or $statusCode -ge 300) {
        if ($Operation -eq 'CreateSubmission' -and $statusCode -eq 409) {
            throw [System.InvalidOperationException]::new(
                'Partner Center rejected submission creation (HTTP 409). Inspect Partner Center for an existing draft or an unsupported app setting before retrying.'
            )
        }
        throw [System.InvalidOperationException]::new(
            "The Store submission request failed during $Operation (HTTP $statusCode). Inspect Partner Center before retrying."
        )
    }

    return $response
}

function ConvertFrom-MsStoreJsonResponse {
    param([Parameter(Mandatory)][string]$Content)

    try {
        $value = ConvertFrom-Json -InputObject $Content -AsHashtable -Depth 100
    } catch {
        throw [System.InvalidOperationException]::new('The Store submission service returned an invalid response.')
    }
    if ($value -isnot [System.Collections.IDictionary]) {
        throw [System.InvalidOperationException]::new('The Store submission service returned an invalid response.')
    }
    return $value
}

function Add-MsStoreUriQuery {
    param(
        [Parameter(Mandatory)][string]$BaseUri,
        [Parameter(Mandatory)][string]$Query
    )

    if ($BaseUri.Contains('#')) {
        throw [System.InvalidOperationException]::new('The Store package upload URL was invalid.')
    }
    $separator = if ($BaseUri.Contains('?')) { '&' } else { '?' }
    return "$BaseUri$separator$Query"
}

function New-MsStoreZipArchive {
    param([Parameter(Mandatory)][string]$PackagePath)

    $packageName = [System.IO.Path]::GetFileName($PackagePath)
    if ($packageName -cnotmatch '^[A-Za-z0-9][A-Za-z0-9._-]*\.msix$') {
        throw 'The Microsoft Store package filename is invalid.'
    }

    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    $allowedTempPrefix = $tempRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    $archivePath = [System.IO.Path]::GetFullPath(
        (Join-Path $tempRoot "teleark-store-submission-$([guid]::NewGuid().ToString('N')).zip")
    )
    if (-not $archivePath.StartsWith($allowedTempPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to create a Store package archive outside the temporary directory.'
    }

    $archiveFile = $null
    $archive = $null
    $source = $null
    $entryStream = $null
    try {
        $archiveFile = [System.IO.File]::Open(
            $archivePath,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None
        )
        $archive = [System.IO.Compression.ZipArchive]::new(
            $archiveFile,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false
        )
        $entry = $archive.CreateEntry($packageName, [System.IO.Compression.CompressionLevel]::NoCompression)
        $entry.LastWriteTime = [DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
        $source = [System.IO.File]::OpenRead($PackagePath)
        $entryStream = $entry.Open()
        $source.CopyTo($entryStream)
        return @{
            Path = $archivePath
            PackageName = $packageName
        }
    } catch {
        if ($null -ne $entryStream) { $entryStream.Dispose(); $entryStream = $null }
        if ($null -ne $source) { $source.Dispose(); $source = $null }
        if ($null -ne $archive) { $archive.Dispose(); $archive = $null }
        if ($null -ne $archiveFile) { $archiveFile.Dispose(); $archiveFile = $null }
        if (Test-Path -LiteralPath $archivePath) {
            Remove-Item -LiteralPath $archivePath -Force -ErrorAction SilentlyContinue
        }
        throw [System.InvalidOperationException]::new('The Microsoft Store package archive could not be created.')
    } finally {
        if ($null -ne $entryStream) { $entryStream.Dispose() }
        if ($null -ne $source) { $source.Dispose() }
        if ($null -ne $archive) { $archive.Dispose() }
        if ($null -ne $archiveFile) { $archiveFile.Dispose() }
    }
}

function Send-MsStoreZipArchive {
    param(
        [Parameter(Mandatory)][string]$UploadUri,
        [Parameter(Mandatory)][string]$ArchivePath,
        [Parameter(Mandatory)][scriptblock]$Transport
    )

    $parsedUploadUri = $null
    if (-not [Uri]::TryCreate($UploadUri, [UriKind]::Absolute, [ref]$parsedUploadUri) -or
        $parsedUploadUri.Scheme -cne 'https' -or
        $parsedUploadUri.Host -notmatch '(?i)\.blob\.core\.windows\.net$' -or
        -not [string]::IsNullOrEmpty($parsedUploadUri.UserInfo) -or
        $parsedUploadUri.Fragment) {
        throw [System.InvalidOperationException]::new('The Store package upload URL was invalid.')
    }

    $stream = $null
    $blockIds = [System.Collections.Generic.List[string]]::new()
    try {
        $stream = [System.IO.File]::OpenRead($ArchivePath)
        $buffer = [byte[]]::new($script:MsStoreUploadBlockSize)
        $index = 0
        while (($read = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
            if ($index -ge $script:MsStoreMaximumBlockCount) {
                throw [System.InvalidOperationException]::new('The Store package archive exceeds the supported upload size.')
            }

            $blockId = 'teleark-{0:D8}' -f $index
            $blockIdBase64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($blockId))
            $blockIdQuery = [Uri]::EscapeDataString($blockIdBase64)
            $blockUri = Add-MsStoreUriQuery -BaseUri $UploadUri -Query "comp=block&blockid=$blockIdQuery"
            $blockBytes = [byte[]]::new($read)
            [Buffer]::BlockCopy($buffer, 0, $blockBytes, 0, $read)
            $request = @{
                Method = 'PUT'
                Uri = $blockUri
                Headers = @{ 'x-ms-version' = $script:AzureBlobApiVersion }
                ContentType = 'application/octet-stream'
                Bytes = $blockBytes
            }
            $null = Invoke-MsStoreSafeRequest -Request $request -Transport $Transport -Operation 'UploadPackageBlock'
            $blockIds.Add($blockIdBase64)
            $index++
        }
    } catch {
        if ($_.Exception.Message -match 'exceeds the supported upload size') { throw }
        if ($_.Exception.Message -match 'in-progress draft may remain') { throw }
        $statusMatch = [regex]::Match($_.Exception.Message, '(?i)\bHTTP\s+(?<status>[1-5][0-9]{2})\b')
        if ($statusMatch.Success) {
            throw [System.InvalidOperationException]::new(
                "The Store package upload failed (HTTP $($statusMatch.Groups['status'].Value)). An in-progress draft may remain; inspect Partner Center before retrying."
            )
        }
        throw [System.InvalidOperationException]::new(
            'The Store package upload failed. An in-progress draft may remain; inspect Partner Center before retrying.'
        )
    } finally {
        if ($null -ne $stream) { $stream.Dispose() }
    }

    if ($blockIds.Count -eq 0) {
        throw [System.InvalidOperationException]::new('The Store package archive was empty.')
    }

    $xml = [System.Text.StringBuilder]::new()
    $null = $xml.Append('<?xml version="1.0" encoding="utf-8"?><BlockList>')
    foreach ($blockId in $blockIds) {
        $null = $xml.Append('<Latest>').Append($blockId).Append('</Latest>')
    }
    $null = $xml.Append('</BlockList>')
    $blockListUri = Add-MsStoreUriQuery -BaseUri $UploadUri -Query 'comp=blocklist'
    $blockListRequest = @{
        Method = 'PUT'
        Uri = $blockListUri
        Headers = @{ 'x-ms-version' = $script:AzureBlobApiVersion }
        ContentType = 'application/xml; charset=utf-8'
        Body = $xml.ToString()
    }
    $null = Invoke-MsStoreSafeRequest -Request $blockListRequest -Transport $Transport -Operation 'CommitPackageUpload'
}

function Invoke-MsStoreSubmission {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$TenantId,
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$ClientId,
        [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string]$ProductId,
        [Parameter(Mandatory)][ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })][string]$PackagePath,
        [scriptblock]$HttpTransport,
        [scriptblock]$SleepAction,
        [ValidateRange(1, 3600)][int]$PollTimeoutSeconds = 900,
        [ValidateRange(0, 60)][int]$PollIntervalSeconds = 10
    )

    $packagePathFull = [System.IO.Path]::GetFullPath($PackagePath)
    if ([System.IO.Path]::GetExtension($packagePathFull) -cne '.msix') {
        throw 'The Microsoft Store submission requires one .msix package.'
    }
    if ($null -eq $HttpTransport) {
        $HttpTransport = { param($request) Invoke-MsStoreHttpRequest -Request $request }.GetNewClosure()
    }
    if ($null -eq $SleepAction) {
        $SleepAction = { param($seconds) Start-Sleep -Seconds $seconds }.GetNewClosure()
    }

    $tenantSegment = [Uri]::EscapeDataString($TenantId)
    $applicationSegment = [Uri]::EscapeDataString($ProductId)
    $tokenUri = "https://login.microsoftonline.com/$tenantSegment/oauth2/token"
    $clientSecret = [System.Environment]::GetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', 'Process')
    if ([string]::IsNullOrWhiteSpace($clientSecret)) {
        throw 'Required Microsoft Store submission setting AZURE_AD_APPLICATION_SECRET is missing.'
    }
    $tokenBody = $null
    try {
        $tokenBody = ConvertTo-MsStoreFormBody -Values ([ordered]@{
            grant_type = 'client_credentials'
            client_id = $ClientId
            client_secret = $clientSecret
            resource = 'https://manage.devcenter.microsoft.com'
        })
        $tokenRequest = @{
            Method = 'POST'
            Uri = $tokenUri
            Headers = @{}
            ContentType = 'application/x-www-form-urlencoded'
            Body = $tokenBody
        }
        $tokenResponse = Invoke-MsStoreSafeRequest -Request $tokenRequest -Transport $HttpTransport -Operation 'TokenRequest'
    } finally {
        $clientSecret = $null
        $tokenBody = $null
        $tokenRequest = $null
        [System.Environment]::SetEnvironmentVariable('AZURE_AD_APPLICATION_SECRET', $null, 'Process')
    }
    $tokenData = ConvertFrom-MsStoreJsonResponse -Content ([string]$tokenResponse.Content)
    if (-not $tokenData.Contains('access_token') -or [string]::IsNullOrWhiteSpace([string]$tokenData['access_token'])) {
        throw [System.InvalidOperationException]::new('Microsoft identity did not return an access token.')
    }
    $accessToken = [string]$tokenData['access_token']
    $authorizationHeaders = @{ Authorization = "Bearer $accessToken" }
    $submissionsUri = "$script:MsStoreSubmissionApiBaseUri/$applicationSegment/submissions"

    $archive = New-MsStoreZipArchive -PackagePath $packagePathFull
    try {
        $createdResponse = Invoke-MsStoreSafeRequest -Request @{
            Method = 'POST'
            Uri = $submissionsUri
            Headers = $authorizationHeaders
        } -Transport $HttpTransport -Operation 'CreateSubmission'
        $created = ConvertFrom-MsStoreJsonResponse -Content ([string]$createdResponse.Content)
        if (-not $created.Contains('id') -or -not $created.Contains('fileUploadUrl')) {
            throw [System.InvalidOperationException]::new(
                'Partner Center returned an incomplete submission draft. Inspect Partner Center before retrying.'
            )
        }
        $submissionId = [string]$created['id']
        if ($submissionId -cnotmatch '^[A-Za-z0-9-]+$') {
            throw [System.InvalidOperationException]::new(
                'Partner Center returned an invalid submission draft. Inspect Partner Center before retrying.'
            )
        }
        $uploadUri = [string]$created['fileUploadUrl']
    } catch {
        $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
        $archiveFullPath = [System.IO.Path]::GetFullPath([string]$archive.Path)
        if ($archiveFullPath.StartsWith(($tempRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar), [System.StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $archiveFullPath -Force -ErrorAction SilentlyContinue
        }
        throw
    }
    $writableFields = @(
        'applicationCategory', 'pricing', 'visibility', 'targetPublishMode', 'targetPublishDate',
        'listings', 'hardwarePreferences', 'automaticBackupEnabled', 'canInstallOnRemovableMedia',
        'isGameDvrEnabled', 'gamingOptions', 'hasExternalInAppProducts', 'meetAccessibilityGuidelines',
        'notesForCertification', 'applicationPackages', 'packageDeliveryOptions', 'enterpriseLicensing',
        'allowMicrosoftDecideAppAvailabilityToFutureDeviceFamilies', 'allowTargetFutureDeviceFamilies',
        'trailers'
    )
    $updatedSubmission = [ordered]@{}
    foreach ($field in $writableFields) {
        if (-not $created.Contains($field)) { continue }
        if ($field -ceq 'pricing' -and $created[$field] -is [System.Collections.IDictionary]) {
            $pricing = [ordered]@{}
            foreach ($pricingField in @('trialPeriod', 'marketSpecificPricings', 'priceId')) {
                if ($created[$field].Contains($pricingField)) {
                    $pricing[$pricingField] = $created[$field][$pricingField]
                }
            }
            $updatedSubmission[$field] = $pricing
            continue
        }
        $updatedSubmission[$field] = $created[$field]
    }

    try {
        $updatedSubmission['applicationPackages'] = @(
            [ordered]@{
                fileName = $archive.PackageName
                fileStatus = 'PendingUpload'
                minimumDirectXVersion = 'None'
                minimumSystemRam = 'None'
            }
        )
        $updateBody = ConvertTo-Json -InputObject $updatedSubmission -Depth 100 -Compress
        $submissionUri = "$submissionsUri/$submissionId"
        $null = Invoke-MsStoreSafeRequest -Request @{
            Method = 'PUT'
            Uri = $submissionUri
            Headers = $authorizationHeaders
            ContentType = 'application/json; charset=utf-8'
            Body = $updateBody
        } -Transport $HttpTransport -Operation 'UpdateSubmission'

        Send-MsStoreZipArchive -UploadUri $uploadUri -ArchivePath $archive.Path -Transport $HttpTransport

        $null = Invoke-MsStoreSafeRequest -Request @{
            Method = 'POST'
            Uri = "$submissionUri/commit"
            Headers = $authorizationHeaders
        } -Transport $HttpTransport -Operation 'CommitSubmission'

        $deadline = [DateTimeOffset]::UtcNow.AddSeconds($PollTimeoutSeconds)
        $currentStatus = 'CommitStarted'
        while ([DateTimeOffset]::UtcNow -lt $deadline) {
            $statusResponse = Invoke-MsStoreSafeRequest -Request @{
                Method = 'GET'
                Uri = "$submissionUri/status"
                Headers = $authorizationHeaders
            } -Transport $HttpTransport -Operation 'PollSubmissionStatus'
            $statusData = ConvertFrom-MsStoreJsonResponse -Content ([string]$statusResponse.Content)
            if (-not $statusData.Contains('status')) {
                throw [System.InvalidOperationException]::new('Partner Center returned an invalid submission status.')
            }
            $currentStatus = [string]$statusData['status']
            $successfulStatuses = @('PreProcessing', 'PendingPublication', 'Publishing', 'Published', 'Certification', 'Release')
            $failedStatuses = @('Canceled', 'CommitFailed', 'PreProcessingFailed', 'CertificationFailed', 'ReleaseFailed', 'PublishFailed')
            if ($currentStatus -notin @('None', 'PendingCommit', 'CommitStarted') + $successfulStatuses + $failedStatuses) {
                throw [System.InvalidOperationException]::new('Partner Center returned an unknown submission status.')
            }
            if ($currentStatus -in $failedStatuses) {
                throw [System.InvalidOperationException]::new(
                    "Partner Center rejected the Store submission ($currentStatus). Inspect the draft in Partner Center before retrying."
                )
            }
            if ($currentStatus -in $successfulStatuses) {
                return [pscustomobject]@{ Status = $currentStatus }
            }
            if ($PollIntervalSeconds -gt 0) { & $SleepAction $PollIntervalSeconds }
        }

        throw [System.TimeoutException]::new(
            'Partner Center is still processing the submission commit. Check the active draft in Partner Center before retrying.'
        )
    } catch {
        if ($_.Exception.Message -match 'Partner Center rejected the Store submission|still processing the submission commit|inspect Partner Center|in-progress draft may remain|Store submission request failed') {
            throw
        }
        throw [System.InvalidOperationException]::new(
            'The Store submission did not complete. An in-progress draft may remain; inspect Partner Center before retrying.'
        )
    } finally {
        if ($null -ne $archive -and (Test-Path -LiteralPath $archive.Path)) {
            $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
            $archiveFullPath = [System.IO.Path]::GetFullPath([string]$archive.Path)
            if ($archiveFullPath.StartsWith(($tempRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar), [System.StringComparison]::OrdinalIgnoreCase)) {
                Remove-Item -LiteralPath $archiveFullPath -Force -ErrorAction SilentlyContinue
            }
        }
        $accessToken = $null
    }
}

Export-ModuleMember -Function Invoke-MsStoreSubmission
