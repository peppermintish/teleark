# ISCC.exe in Inno Setup 6 has no usable Windows version resource or --version
# switch. Ask its documented preprocessor for the exact engine version instead.
function Assert-InnoSetupCompilerVersion {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$ExpectedVersion
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Inno Setup compiler is missing: $Path"
    }

    if ($ExpectedVersion -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
        throw "Expected Inno Setup version must have three numeric parts: $ExpectedVersion"
    }
    $expected = [version]::Parse($ExpectedVersion)

    $probeDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "teleark-iscc-version-$([guid]::NewGuid().ToString('N'))"
    $probeScript = Join-Path $probeDirectory 'version-probe.iss'
    try {
        New-Item -ItemType Directory -Path $probeDirectory | Out-Null
        [System.IO.File]::WriteAllLines($probeScript, @(
            "#if Ver != EncodeVer($($expected.Major),$($expected.Minor),$($expected.Build))",
            '#error Installed Inno Setup compiler version does not match the pinned version',
            '#endif',
            '[Setup]',
            'AppName=TeleArk compiler version probe',
            'AppVersion=1.0',
            'DefaultDirName={autopf}\TeleArk Compiler Probe',
            'Output=no',
            'OutputDir=.'
        ))

        $compilerOutput = & $Path $probeScript 2>&1
        if ($LASTEXITCODE -ne 0) {
            $detail = ($compilerOutput | Out-String).Trim()
            throw "ISCC.exe at '$Path' did not pass the Inno Setup $ExpectedVersion version probe (exit code $LASTEXITCODE): $detail"
        }
        return $ExpectedVersion
    } finally {
        if (Test-Path -LiteralPath $probeDirectory) {
            Remove-Item -LiteralPath $probeDirectory -Recurse -Force
        }
    }
}
