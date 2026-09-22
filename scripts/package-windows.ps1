# Assemble Windows release packages (standalone EXE, portable ZIP and EXE setup wizard).
# Run cargo build before invoking this script.
$ErrorActionPreference = 'Stop'

$dry_run = $false
$paths = @()

foreach ($arg in $args) {
    switch ($arg) {
        { $_ -in '--dry-run', '-DryRun', '-dry-run' } { $dry_run = $true; break }
        { $_.StartsWith('--') } {
            [Console]::Error.WriteLine("Usage: scripts/package-windows.ps1 [--dry-run] [binary] [destination_dir] [version] [artifact_label] [architecture]")
            exit 2
        }
        default { $paths += $arg; break }
    }
}

if ($paths.Count -gt 5) {
    [Console]::Error.WriteLine("Usage: scripts/package-windows.ps1 [--dry-run] [binary] [destination_dir] [version] [artifact_label] [architecture]")
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
if ($version -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
    throw "Installer version must be three numeric components: $version"
}
$artifact_label = if ($paths.Count -ge 4) { $paths[3] } else { $version }
if ($artifact_label -cnotmatch '^[A-Za-z0-9][A-Za-z0-9.-]*$') {
    throw "Invalid artifact label: $artifact_label"
}
$architecture = if ($paths.Count -ge 5) { $paths[4] } else { 'x86_64' }
if ($architecture -cne 'x86_64') {
    throw "Unsupported Windows architecture: $architecture"
}

$package_name = "teleark-$artifact_label-windows-$architecture"
$dist_dir = [System.IO.Path]::GetFullPath($dist_dir)
$staging_dir = [System.IO.Path]::GetFullPath((Join-Path $dist_dir $package_name))
if (-not $staging_dir.StartsWith("$dist_dir$([System.IO.Path]::DirectorySeparatorChar)", [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'Staging directory must be within the requested destination directory.'
}
$zip_path = Join-Path $dist_dir "$package_name.zip"
$standalone_path = Join-Path $dist_dir "$package_name.exe"
$setup_base = "TeleArk-Setup-$artifact_label-windows-$architecture"

if ($dry_run) {
    Write-Output "Would assemble Windows portable bundle and installer:"
    Write-Output "  Binary: $binary"
    Write-Output "  Staging: $staging_dir"
    Write-Output "  Portable ZIP: $zip_path"
    Write-Output "  Standalone executable: $standalone_path"
    Write-Output "  Installer: $(Join-Path $dist_dir "$setup_base.exe")"
    Write-Output "Would copy binary, README.md, LICENSE-*, and THIRD_PARTY_NOTICES.md, compress ZIP, and compile a visible EXE setup wizard."
    exit 0
}

if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    [Console]::Error.WriteLine("Binary not found: $binary")
    exit 1
}

# Create staging directory and copy files
if (Test-Path -LiteralPath $staging_dir) {
    Remove-Item -LiteralPath $staging_dir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $staging_dir | Out-Null

Copy-Item -LiteralPath $binary -Destination (Join-Path $staging_dir "teleark.exe") -Force
Copy-Item -LiteralPath $binary -Destination $standalone_path -Force
Copy-Item (Join-Path $repository_root "README.md") $staging_dir -Force
Copy-Item (Join-Path $repository_root "LICENSE-MIT") $staging_dir -Force
Copy-Item (Join-Path $repository_root "LICENSE-APACHE") $staging_dir -Force
Copy-Item (Join-Path $repository_root "THIRD_PARTY_NOTICES.md") $staging_dir -Force

# Create portable ZIP
if (Test-Path -LiteralPath $zip_path) {
    Remove-Item -LiteralPath $zip_path -Force
}
Compress-Archive -Path "$staging_dir\*" -DestinationPath $zip_path -Force
Write-Output "Created portable archive: $zip_path"

$installer_path = Join-Path $dist_dir "$setup_base.exe"
# Keep the transactional payload out of the portable archive and published assets.
$installer_work = Join-Path $dist_dir '.windows-installer'
New-Item -ItemType Directory -Force -Path $installer_work | Out-Null
$msi_path = Join-Path $installer_work 'TeleArk.msi'
& "$PSScriptRoot/build-windows-msi.ps1" -SourceDir $staging_dir -OutputPath $msi_path -Version $version
& "$PSScriptRoot/build-windows-exe.ps1" -MsiPath $msi_path -OutputPath $installer_path
Write-Output "Created installer: $installer_path"

# Generate SHA256 checksums
$checksum_file = Join-Path $dist_dir "SHA256SUMS"
$hash_entries = @()
if (Test-Path $zip_path) {
    $hash = (Get-FileHash -Algorithm SHA256 $zip_path).Hash.ToLower()
    $hash_entries += "$hash  $(Split-Path -Leaf $zip_path)"
}
$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $standalone_path).Hash.ToLower()
$hash_entries += "$hash  $(Split-Path -Leaf $standalone_path)"
if (Test-Path -LiteralPath $installer_path) {
    $hash = (Get-FileHash -Algorithm SHA256 $installer_path).Hash.ToLower()
    $hash_entries += "$hash  $(Split-Path -Leaf $installer_path)"
}
[System.IO.File]::WriteAllLines($checksum_file, $hash_entries, [System.Text.UTF8Encoding]::new($false))
Write-Output "Generated checksums in $checksum_file"
