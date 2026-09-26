# Version resolution uses synthetic Cargo metadata; no compilation or credentials.
$ErrorActionPreference = 'Stop'
$checker = Join-Path $PSScriptRoot 'release-version.ps1'
$expectedManifest = Join-Path (Split-Path -Parent $PSScriptRoot) 'Cargo.toml'
$stage = Join-Path ([IO.Path]::GetTempPath()) "teleark-version-$([guid]::NewGuid().ToString('N'))"
$cases = @(
    @{ Version = '0.5.8'; RefType = 'tag'; RefName = 'v0.5.8'; EventName = 'push'; Pass = $true; Label = '0.5.8' },
    @{ Version = '255.255.65535'; RefType = 'tag'; RefName = 'v255.255.65535'; EventName = 'push'; Pass = $true; Label = '255.255.65535' },
    @{ Version = '0.5.8'; RefType = 'branch'; RefName = 'main'; EventName = 'workflow_dispatch'; Pass = $true; Label = '0.5.8-manual-abcdef0' },
    @{ Version = '0.5.8'; RefType = 'tag'; RefName = 'v0.5.7'; EventName = 'push'; Pass = $false },
    @{ Version = '0.5.8'; RefType = 'branch'; RefName = 'main'; EventName = 'push'; Pass = $false },
    @{ Version = '0.5.8-rc.1'; RefType = 'tag'; RefName = 'v0.5.8-rc.1'; EventName = 'push'; Pass = $false },
    @{ Version = '256.0.0'; RefType = 'tag'; RefName = 'v256.0.0'; EventName = 'push'; Pass = $false },
    @{ Version = '0.256.0'; RefType = 'tag'; RefName = 'v0.256.0'; EventName = 'push'; Pass = $false },
    @{ Version = '0.0.65536'; RefType = 'tag'; RefName = 'v0.0.65536'; EventName = 'push'; Pass = $false },
    @{ Version = '0.5.8'; RefType = 'branch'; RefName = 'main'; EventName = 'workflow_dispatch'; Sha = 'invalid'; Pass = $false },
    @{ Version = '0.5.8'; RefType = 'tag'; RefName = 'v0.5.8'; EventName = 'push'; CargoExit = 7; Pass = $false },
    @{ Version = '0.5.8'; RefType = 'tag'; RefName = 'v0.5.8'; EventName = 'push'; PackageName = 'other'; Pass = $false }
)

function cargo {
    if (($args -join "`n") -cne (@('metadata', '--manifest-path', $expectedManifest, '--format-version', '1', '--no-deps', '--locked') -join "`n")) {
        throw 'Version resolution did not use locked metadata from the repository manifest.'
    }
    $global:LASTEXITCODE = $fixtureCargoExit
    @{ packages = @(@{ name = $fixturePackageName; version = $fixtureVersion }) } | ConvertTo-Json -Depth 4 -Compress
}

try {
    New-Item -ItemType Directory -Path $stage | Out-Null
    Push-Location -LiteralPath $stage
    try {
        foreach ($case in $cases) {
            $fixtureVersion = $case.Version
            $fixturePackageName = if ($case.PackageName) { $case.PackageName } else { 'teleark-gui' }
            $fixtureCargoExit = if ($case.CargoExit) { $case.CargoExit } else { 0 }
            $output = Join-Path $stage 'github-output'
            [IO.File]::WriteAllText($output, "existing=value`n", [Text.UTF8Encoding]::new($false))
            $failure = $null
            try {
                & $checker -RefType $case.RefType -RefName $case.RefName -EventName $case.EventName `
                    -CommitSha $(if ($case.Sha) { $case.Sha } else { 'ABCDEF0123456789' }) -OutputFile $output
            } catch { $failure = $_ }
            if (($null -eq $failure) -ne $case.Pass) { throw "Version validation returned an unexpected result for $($case.Version) / $($case.EventName): $failure" }
            $expected = "existing=value`n"
            if ($case.Pass) { $expected += "version=$($case.Version)`nlabel=$($case.Label)`n" }
            if ([IO.File]::ReadAllText($output) -cne $expected) { throw 'Version validation wrote incorrect or partial outputs.' }
        }
    } finally { Pop-Location }
} finally {
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
$global:LASTEXITCODE = 0
Write-Output "Release version validation passed $($cases.Count) synthetic cases."
