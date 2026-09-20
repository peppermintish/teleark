# Assemble Windows release packages (portable ZIP and Inno Setup installer).
# Run cargo build before invoking this script.
$ErrorActionPreference = 'Stop'

$dry_run = $false
$paths = @()

foreach ($arg in $args) {
    switch ($arg) {
        { $_ -in '--dry-run', '-DryRun', '-dry-run' } { $dry_run = $true; break }
        { $_.StartsWith('--') } {
            [Console]::Error.WriteLine("Usage: scripts/package-windows.ps1 [--dry-run] [binary] [destination_dir] [version]")
            exit 2
        }
        default { $paths += $arg; break }
    }
}

if ($paths.Count -gt 3) {
    [Console]::Error.WriteLine("Usage: scripts/package-windows.ps1 [--dry-run] [binary] [destination_dir] [version]")
    exit 2
}

$repository_root = Split-Path -Parent $PSScriptRoot
$binary = if ($paths.Count -ge 1) { $paths[0] } else { Join-Path $repository_root "target\release\teleark.exe" }
$dist_dir = if ($paths.Count -ge 2) { $paths[1] } else { Join-Path $repository_root "dist" }

# Extract version from Cargo.toml if not passed
$version = if ($paths.Count -ge 3) {
    $paths[2]
} else {
    $cargo_content = Get-Content (Join-Path $repository_root "Cargo.toml") -Raw
    if ($cargo_content -match '(?m)^version\s*=\s*"([^"]+)"') {
        $matches[1]
    } else {
        "manual"
    }
}
$version = $version -replace '/', '-'

$package_name = "teleark-$version-windows-x86_64"
$staging_dir = Join-Path $dist_dir $package_name
$zip_path = Join-Path $dist_dir "$package_name.zip"
$setup_base = "TeleArk-Setup-$version-windows-x86_64"

if ($dry_run) {
    Write-Output "Would assemble Windows portable bundle and installer:"
    Write-Output "  Binary: $binary"
    Write-Output "  Staging: $staging_dir"
    Write-Output "  Portable ZIP: $zip_path"
    Write-Output "  Installer: $(Join-Path $dist_dir "$setup_base.exe")"
    Write-Output "Would copy binary, README.md, LICENSE-*, and THIRD_PARTY_NOTICES.md, compress ZIP, and run ISCC.exe."
    exit 0
}

if (-not (Test-Path $binary)) {
    [Console]::Error.WriteLine("Binary not found: $binary")
    exit 1
}

# Create staging directory and copy files
if (Test-Path $staging_dir) {
    Remove-Item -Recurse -Force $staging_dir
}
New-Item -ItemType Directory -Force -Path $staging_dir | Out-Null

Copy-Item $binary (Join-Path $staging_dir "teleark.exe") -Force
Copy-Item (Join-Path $repository_root "README.md") $staging_dir -Force
Copy-Item (Join-Path $repository_root "LICENSE-MIT") $staging_dir -Force
Copy-Item (Join-Path $repository_root "LICENSE-APACHE") $staging_dir -Force
Copy-Item (Join-Path $repository_root "THIRD_PARTY_NOTICES.md") $staging_dir -Force

# Create portable ZIP
if (Test-Path $zip_path) {
    Remove-Item -Force $zip_path
}
Compress-Archive -Path "$staging_dir\*" -DestinationPath $zip_path -Force
Write-Output "Created portable archive: $zip_path"

# Find Inno Setup compiler (ISCC.exe)
$iscc = $null
if (Get-Command "ISCC.exe" -ErrorAction SilentlyContinue) {
    $iscc = "ISCC.exe"
} else {
    $search_paths = @(
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe",
        "C:\Program Files (x86)\Inno Setup 5\ISCC.exe"
    )
    foreach ($candidate in $search_paths) {
        if (Test-Path $candidate) {
            $iscc = $candidate
            break
        }
    }
}

if ($iscc) {
    $iss_file = Join-Path $repository_root "scripts\teleark.iss"
    Write-Output "Compiling Inno Setup installer with $iscc..."
    & $iscc "/DAppVersion=$version" "/DSourceDir=$staging_dir" "/DOutputDir=$dist_dir" "/DOutputBaseFilename=$setup_base" $iss_file
    if ($LASTEXITCODE -ne 0) {
        [Console]::Error.WriteLine("Inno Setup compilation failed with code $LASTEXITCODE")
        exit $LASTEXITCODE
    }
    Write-Output "Created installer: $(Join-Path $dist_dir "$setup_base.exe")"
} else {
    Write-Warning "Inno Setup compiler (ISCC.exe) was not found on PATH or standard install paths. Skipping installer generation."
}

# Generate SHA256 checksums
$checksum_file = Join-Path $dist_dir "SHA256SUMS"
$hash_entries = @()
if (Test-Path $zip_path) {
    $hash = (Get-FileHash -Algorithm SHA256 $zip_path).Hash.ToLower()
    $hash_entries += "$hash  $(Split-Path -Leaf $zip_path)"
}
$installer_path = Join-Path $dist_dir "$setup_base.exe"
if (Test-Path $installer_path) {
    $hash = (Get-FileHash -Algorithm SHA256 $installer_path).Hash.ToLower()
    $hash_entries += "$hash  $(Split-Path -Leaf $installer_path)"
}
if ($hash_entries.Count -gt 0) {
    $hash_entries | Set-Content $checksum_file -Encoding utf8
    Write-Output "Generated checksums in $checksum_file"
}
