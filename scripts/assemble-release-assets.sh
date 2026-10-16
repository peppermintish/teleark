#!/usr/bin/env bash
set -euo pipefail

artifact_root="${1:?downloaded artifact root is required}"
destination="${2:?release destination is required}"
label="${3:?release artifact label is required}"

if [[ ! "$label" =~ ^[A-Za-z0-9][A-Za-z0-9.-]*$ ]]; then
  echo 'Invalid release artifact label.' >&2
  exit 2
fi

artifact_root="$(cd "$artifact_root" && pwd)"
repository_root="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -e "$destination" ]]; then
  if [[ ! -d "$destination" ]] || find "$destination" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
    echo 'Release destination must be a new or empty directory.' >&2
    exit 1
  fi
fi

legal_assets=(LICENSE-MIT LICENSE-APACHE THIRD_PARTY_NOTICES.md)
for asset in "${legal_assets[@]}"; do
  if [[ ! -s "$repository_root/$asset" ]]; then
    echo "Required release notice is missing or empty: $asset" >&2
    exit 1
  fi
done

declare -A packages_by_group=(
  [release-windows-x86_64]="TeleArk-${label}-windows-x86_64.msix"
  [release-macos-universal]="teleark-${label}-macos-universal.bin TeleArk-${label}-macos-universal.pkg teleark-${label}-macos-universal.tar.gz"
  [release-linux-x86_64]="teleark-${label}-linux-x86_64.AppImage teleark-${label}-linux-x86_64.tar.gz teleark_${label}_amd64.deb"
)

expected_groups="$(printf '%s\n' "${!packages_by_group[@]}" | LC_ALL=C sort)"
actual_groups="$(find "$artifact_root" -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort)"
if [[ "$actual_groups" != "$expected_groups" ]]; then
  echo 'Downloaded release artifact groups do not match the required platform set.' >&2
  exit 1
fi

for group in "${!packages_by_group[@]}"; do
  group_directory="$artifact_root/$group"
  if [[ ! -d "$group_directory" ]]; then
    echo "Missing downloaded artifact group: $group" >&2
    exit 1
  fi

  read -r -a packages <<< "${packages_by_group[$group]}"
  expected_entries="$(printf '%s\n' SHA256SUMS "${packages[@]}" | LC_ALL=C sort)"
  actual_entries="$(find "$group_directory" -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort)"
  if [[ "$actual_entries" != "$expected_entries" ]]; then
    echo "Downloaded files in $group do not match its required package set and checksum manifest." >&2
    exit 1
  fi

  for package in "${packages[@]}"; do
    if [[ ! -s "$group_directory/$package" ]]; then
      echo "Downloaded package is missing or empty: $group/$package" >&2
      exit 1
    fi
  done

  declare -A expected_names=()
  for package in "${packages[@]}"; do expected_names["$package"]=1; done
  declare -A checksum_names=()
  while IFS= read -r line || [[ -n "$line" ]]; do
    # Windows PowerShell WriteAllLines emits CRLF. Strip only a final CR so
    # the filename and checksum syntax are still validated exactly.
    line="${line%$'\r'}"
    if [[ ! "$line" =~ ^([[:xdigit:]]{64})[[:space:]][[:space:]]([^/[:space:]]+)$ ]]; then
      echo "Malformed checksum entry in $group/SHA256SUMS." >&2
      exit 1
    fi
    name="${BASH_REMATCH[2]}"
    if [[ -z "${expected_names[$name]+present}" || -n "${checksum_names[$name]+present}" ]]; then
      echo "Unexpected or duplicate checksum entry in $group/SHA256SUMS." >&2
      exit 1
    fi
    checksum_names["$name"]=1
  done < "$group_directory/SHA256SUMS"

  if [[ "${#checksum_names[@]}" -ne "${#packages[@]}" ]]; then
    echo "Checksum manifest in $group does not cover each required package exactly once." >&2
    exit 1
  fi
  if ! (cd "$group_directory" && sed 's/\r$//' SHA256SUMS | sha256sum --quiet -c -); then
    echo "Downloaded checksum verification failed for $group." >&2
    exit 1
  fi
done

mkdir -p "$destination"
destination="$(cd "$destination" && pwd)"
all_packages=()
for group in "${!packages_by_group[@]}"; do
  read -r -a packages <<< "${packages_by_group[$group]}"
  for package in "${packages[@]}"; do
    cp -- "$artifact_root/$group/$package" "$destination/$package"
    all_packages+=("$package")
  done
done

for asset in "${legal_assets[@]}"; do
  cp -- "$repository_root/$asset" "$destination/$asset"
done

(cd "$destination" && find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%f\0' | LC_ALL=C sort -z | xargs -0 sha256sum > SHA256SUMS)
(cd "$destination" && sha256sum --quiet -c SHA256SUMS)

expected_release_files="$(printf '%s\n' SHA256SUMS "${all_packages[@]}" "${legal_assets[@]}" | LC_ALL=C sort)"
actual_release_files="$(find "$destination" -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort)"
release_asset_count="$(printf '%s\n' "$actual_release_files" | wc -l | tr -d '[:space:]')"
if [[ "$release_asset_count" != 11 || "$actual_release_files" != "$expected_release_files" ]]; then
  echo 'Assembled release did not contain exactly seven platform packages and four accompanying assets.' >&2
  exit 1
fi

echo 'Verified downloaded package checksums and assembled seven platform packages plus three legal notices and SHA256SUMS.'
