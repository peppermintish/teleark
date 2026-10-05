#!/usr/bin/env python3
"""Build the dependency-free public site from validated GitHub release metadata."""

import argparse
import html
import json
from pathlib import Path
import re
import shutil
import sys
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "site"
REPOSITORY = "https://github.com/peppermintish/teleark"
LATEST_API = "https://api.github.com/repos/peppermintish/teleark/releases/latest"
MAX_METADATA_BYTES = 2 * 1024 * 1024


def release_values(release):
    """Only publish a stable release and the exact official platform artifacts."""
    tag = release.get("tag_name", "")
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("Expected a stable vX.Y.Z release tag")
    if release.get("draft") is not False or release.get("prerelease") is not False:
        raise ValueError("Drafts and prereleases are not website downloads")
    version = tag[1:]
    release_url = f"{REPOSITORY}/releases/tag/{tag}"
    if release.get("html_url") != release_url:
        raise ValueError("Release must belong to the TeleArk repository")
    names = {
        "linux_appimage": f"teleark-{version}-linux-x86_64.AppImage",
        "linux_deb": f"teleark_{version}_amd64.deb",
        "linux_archive": f"teleark-{version}-linux-x86_64.tar.gz",
        "macos_pkg": f"TeleArk-{version}-macos-universal.pkg",
        "macos_archive": f"teleark-{version}-macos-universal.tar.gz",
        "macos_binary": f"teleark-{version}-macos-universal.bin",
        "checksums": "SHA256SUMS",
    }
    assets = {}
    for asset in release.get("assets", []):
        name = asset.get("name")
        if name in assets:
            raise ValueError("Duplicate release asset")
        assets[name] = asset.get("browser_download_url")
    values = {"release_tag": tag, "release_url": release_url}
    for key, name in names.items():
        expected_url = f"{REPOSITORY}/releases/download/{tag}/{name}"
        if assets.get(name) != expected_url:
            raise ValueError(f"Missing or noncanonical asset: {key}")
        values[f"{key}_url"] = expected_url
        values[f"{key}_name"] = name
    return values


def fetch_release():
    request = urllib.request.Request(
        LATEST_API,
        headers={"Accept": "application/vnd.github+json", "User-Agent": "TeleArk-Pages"},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        payload = response.read(MAX_METADATA_BYTES + 1)
    if len(payload) > MAX_METADATA_BYTES:
        raise ValueError("Release metadata exceeds the supported size")
    return json.loads(payload)


def render(template, values):
    def substitute(match):
        key = match.group(1)
        if key not in values:
            raise ValueError(f"Unknown site template value: {key}")
        return html.escape(values[key], quote=True)

    return re.sub(r"\{\{(\w+)\}\}", substitute, template)


def build(release, output):
    values = release_values(release)
    document = render((SOURCE / "index.html").read_text(encoding="utf-8"), values)
    output = output.resolve()
    if output == ROOT or output == SOURCE or SOURCE in output.parents or output in ROOT.parents:
        raise ValueError("Build output must be separate from the source tree")
    output.mkdir(parents=True, exist_ok=True)
    # Explicit public allow-list: private repository files never enter a Pages artifact.
    for name in ("styles.css", "404.html", "robots.txt", "sitemap.xml"):
        shutil.copyfile(SOURCE / name, output / name)
    shutil.copytree(SOURCE / "assets", output / "assets", dirs_exist_ok=True)
    temporary = output / "index.html.tmp"
    temporary.write_text(document, encoding="utf-8", newline="\n")
    temporary.replace(output / "index.html")
    (output / ".nojekyll").touch()
    return values["release_tag"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--refresh-release", action="store_true", help="Fetch the latest stable public release; fail on errors")
    selection.add_argument("--release-json", type=Path, default=SOURCE / "release.json", help="Offline release metadata (default: checked-in snapshot)")
    parser.add_argument("--output", type=Path, default=ROOT / "target" / "site-preview")
    args = parser.parse_args()
    try:
        if args.refresh_release:
            print("Checking the latest published GitHub release...", flush=True)
            release = fetch_release()
        else:
            print("Reading the offline release snapshot...", flush=True)
            release = json.loads(args.release_json.read_text(encoding="utf-8"))
        print("Validating platform downloads and building the public site...", flush=True)
        tag = build(release, args.output)
    except (OSError, ValueError, TypeError, AttributeError, urllib.error.URLError):
        print("Site build failed: check release availability, required assets and output access. No deployment was performed.", file=sys.stderr)
        return 1
    print(f"Site ready: {tag} - {args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
