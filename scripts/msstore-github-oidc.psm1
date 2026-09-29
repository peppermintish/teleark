function Get-MsStoreGitHubOidcToken {
    [CmdletBinding()]
    param(
        [string]$RequestUrl = $env:ACTIONS_ID_TOKEN_REQUEST_URL,
        [string]$RequestToken = $env:ACTIONS_ID_TOKEN_REQUEST_TOKEN,
        [scriptblock]$HttpGet
    )

    if ([string]::IsNullOrWhiteSpace($RequestUrl) -or [string]::IsNullOrWhiteSpace($RequestToken)) {
        throw 'GitHub Actions did not provide its OIDC token request endpoint. Grant id-token: write to the Store submission job.'
    }

    $requestUri = $null
    if (-not [uri]::TryCreate($RequestUrl, [System.UriKind]::Absolute, [ref]$requestUri) -or $requestUri.Scheme -ne 'https') {
        throw 'GitHub Actions provided an invalid OIDC token request endpoint.'
    }

    $uriBuilder = [System.UriBuilder]::new($requestUri)
    $query = $uriBuilder.Query.TrimStart('?')
    if ($query -match '(?:^|&)audience=') {
        throw 'The GitHub Actions OIDC token request endpoint already has an audience parameter.'
    }

    $audience = 'api://AzureADTokenExchange'
    $encodedAudience = [uri]::EscapeDataString($audience)
    $uriBuilder.Query = if ($query) { "$query&audience=$encodedAudience" } else { "audience=$encodedAudience" }
    $headers = @{ Authorization = "Bearer $RequestToken" }

    try {
        if ($HttpGet) {
            $response = & $HttpGet $uriBuilder.Uri $headers
        } else {
            $response = Invoke-RestMethod -Method Get -Uri $uriBuilder.Uri.AbsoluteUri -Headers $headers -ErrorAction Stop
        }
    } catch {
        throw 'GitHub Actions OIDC token request failed.'
    }

    $token = [string]$response.value
    if ([string]::IsNullOrWhiteSpace($token)) {
        throw 'GitHub Actions returned an empty OIDC token.'
    }

    return $token
}

function Invoke-MsStoreWithGitHubOidc {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [ValidateNotNullOrEmpty()]
        [string[]]$ArgumentList,

        [string]$CliCommand = 'msstore',

        [scriptblock]$TokenProvider
    )

    $previousAssertion = [System.Environment]::GetEnvironmentVariable('MSSTORE_CLIENT_ASSERTION', 'Process')
    try {
        $assertion = if ($TokenProvider) {
            & $TokenProvider
        } else {
            Get-MsStoreGitHubOidcToken
        }

        if ([string]::IsNullOrWhiteSpace([string]$assertion)) {
            throw 'GitHub Actions returned an empty OIDC token.'
        }

        [System.Environment]::SetEnvironmentVariable('MSSTORE_CLIENT_ASSERTION', [string]$assertion, 'Process')
        [System.Environment]::SetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_TOKEN', $null, 'Process')
        [System.Environment]::SetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_URL', $null, 'Process')

        & $CliCommand @ArgumentList
        $exitCode = $LASTEXITCODE

        if ($null -eq $exitCode) { $exitCode = 0 }
        if ([int]$exitCode -ne 0) {
            throw "Microsoft Store CLI failed with exit code $exitCode."
        }
    } finally {
        [System.Environment]::SetEnvironmentVariable('MSSTORE_CLIENT_ASSERTION', $previousAssertion, 'Process')
        [System.Environment]::SetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_TOKEN', $null, 'Process')
        [System.Environment]::SetEnvironmentVariable('ACTIONS_ID_TOKEN_REQUEST_URL', $null, 'Process')
    }
}

Export-ModuleMember -Function Get-MsStoreGitHubOidcToken, Invoke-MsStoreWithGitHubOidc
