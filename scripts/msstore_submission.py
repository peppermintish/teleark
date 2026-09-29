"""MSIX updates using Microsoft's documented Dev Center API; standard library only.

Contract: https://learn.microsoft.com/en-us/windows/uwp/monetize/manage-app-submissions
Credentials are read from the process environment, never from command arguments.
"""

from __future__ import annotations

import argparse
import base64
import copy
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import socket
import ssl
import subprocess
import sys
import tempfile
import time
from urllib.parse import quote, urlencode, urlsplit
import xml.etree.ElementTree as ET
import zipfile


API = "https://manage.devcenter.microsoft.com/v1.0/my/applications"
BLOCK_SIZE = 16 * 1024 * 1024
MAX_RESPONSE = 16 * 1024 * 1024
ACCEPTED = {"PreProcessing", "Certification", "Release", "PendingPublication", "Publishing", "Published"}
FAILED = {"Canceled", "CommitFailed", "PreProcessingFailed", "CertificationFailed", "ReleaseFailed", "PublishFailed"}
WAITING = {"None", "PendingCommit", "CommitStarted"}
WRITABLE = {
    "applicationCategory", "pricing", "visibility", "targetPublishMode", "targetPublishDate",
    "listings", "hardwarePreferences", "automaticBackupEnabled", "canInstallOnRemovableMedia",
    "isGameDvrEnabled", "gamingOptions", "hasExternalInAppProducts", "meetAccessibilityGuidelines",
    "notesForCertification", "applicationPackages", "packageDeliveryOptions", "enterpriseLicensing",
    "allowMicrosoftDecideAppAvailabilityToFutureDeviceFamilies", "allowTargetFutureDeviceFamilies", "trailers",
}


class StoreError(Exception):
    """An operator-safe error, containing no response bodies, URLs or credentials."""


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def emit(phase: str) -> None:
    print(f"Microsoft Store: {phase}.", flush=True)


class Journal:
    """Versioned, bounded diagnostic receipt. Contains no remote metadata or secrets."""

    def __init__(self, path: Path, package: Path):
        self.path = path
        self.data = {
            "schema_version": 1, "package_name": package.name, "package_sha256": sha256(package),
            "phase": "prepared", "submission_id": None, "status": None,
            "events": [], "omitted_events": 0,
        }

    def record(self, phase: str, **values) -> None:
        self.data.update(values, phase=phase)
        self.data["events"].append({"phase": phase, "utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())})
        if len(self.data["events"]) > 64:
            self.data["events"].pop(0)
            self.data["omitted_events"] += 1
        emit(phase)
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            temporary = self.path.with_suffix(".tmp")
            temporary.write_text(json.dumps(self.data, indent=2) + "\n", encoding="utf-8")
            temporary.replace(self.path)
        except OSError:
            print("Microsoft Store: diagnostic receipt could not be saved.", flush=True)


class HttpTransport:
    def request(self, method: str, uri: str, headers: dict, body: bytes | None = None):
        parsed = urlsplit(uri)
        if parsed.scheme != "https" or parsed.username or parsed.password or parsed.fragment or parsed.port not in (None, 443):
            raise StoreError("Invalid HTTPS destination.")
        connection = http.client.HTTPSConnection(parsed.hostname, timeout=120)
        try:
            path = parsed.path + (f"?{parsed.query}" if parsed.query else "")
            connection.request(method, path, body=body, headers=headers)
            response = connection.getresponse()
            content = response.read(MAX_RESPONSE + 1)
            if len(content) > MAX_RESPONSE:
                raise StoreError("Response exceeded the supported size.")
            return response.status, content
        finally:
            connection.close()


def request(transport, operation: str, method: str, uri: str, headers=None, body=None, content_type=None):
    headers = dict(headers or {})
    if content_type:
        headers["Content-Type"] = content_type
    try:
        status, content = transport.request(method, uri, headers, body)
    except Exception as failure:
        category = "other"
        if isinstance(failure, (TimeoutError, socket.timeout)):
            category = "timeout"
        elif isinstance(failure, ssl.SSLError):
            category = "tls"
        elif isinstance(failure, socket.gaierror):
            category = "dns"
        elif isinstance(failure, OSError):
            category = "socket"
        raise StoreError(
            f"{operation} failed (transport={category}). Check the submission receipt and Partner Center before retrying."
        ) from None
    if not 200 <= status < 300:
        guidance = {
            400: "Check the API configuration and submission requirements.",
            401: "Check the Entra application secret and token audience.",
            403: "Check the Entra application's Partner Center Manager access.",
            404: "Check the Store Product ID and existing submission.",
            409: "An active draft or unsupported Partner Center setting prevents this operation.",
            429: "The service is rate limiting requests; wait before retrying.",
        }.get(status, "Check Partner Center before retrying.")
        if status in (401, 403) and operation != "TokenRequest":
            guidance = (
                "Verify the token audience and the exact tenant/client application associated with "
                "this Windows developer account in Partner Center with the Manager role."
            )
        raise StoreError(f"{operation} failed (HTTP {status}). {guidance}")
    return content


def json_response(content: bytes) -> dict:
    try:
        value = json.loads(content)
        if not isinstance(value, dict):
            raise ValueError()
        return value
    except (ValueError, UnicodeError):
        raise StoreError("The service returned invalid JSON.") from None


def submission_id(draft: dict) -> str:
    value = draft.get("id", "")
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9-]{1,80}", value):
        raise StoreError("Partner Center returned an invalid submission ID.")
    return value


def update_payload(draft: dict, package_name: str, digest: str) -> dict:
    payload = {key: copy.deepcopy(value) for key, value in draft.items() if key in WRITABLE}
    if "pricing" in payload:
        pricing = payload["pricing"]
        # Pricing Version 2 is not writable through this API. Package-only updates
        # omit it, as Microsoft documents; ordinary legacy pricing remains intact.
        if not isinstance(pricing, dict) or not re.fullmatch(r"Free|Tier[0-9]+", str(pricing.get("priceId", ""))):
            del payload["pricing"]
        else:
            payload["pricing"] = {key: value for key, value in pricing.items() if key in {"priceId", "trialPeriod", "marketSpecificPricings"}}
    packages = []
    for previous in draft.get("applicationPackages", []):
        if previous.get("fileName") == package_name:
            continue
        packages.append({
            "fileName": previous["fileName"], "fileStatus": "PendingDelete",
            "minimumDirectXVersion": previous.get("minimumDirectXVersion", "None"),
            "minimumSystemRam": previous.get("minimumSystemRam", "None"),
        })
    packages.append({"fileName": package_name, "fileStatus": "PendingUpload", "minimumDirectXVersion": "None", "minimumSystemRam": "None"})
    payload["applicationPackages"] = packages
    marker = f"TeleArk CI package SHA256: {digest}"
    notes = str(draft.get("notesForCertification") or "")
    payload["notesForCertification"] = notes if marker in notes.splitlines() else f"{notes}\n{marker}".strip()
    return payload


def upload_archive(transport, upload_uri: str, archive: Path, journal: Journal) -> None:
    parsed = urlsplit(upload_uri)
    if (parsed.scheme != "https" or not parsed.hostname or not parsed.hostname.endswith(".blob.core.windows.net")
            or parsed.username or parsed.password or parsed.fragment or parsed.port not in (None, 443) or not parsed.query):
        raise StoreError("Partner Center returned an invalid Azure Blob upload destination.")
    headers = {"x-ms-version": "2023-11-03"}
    separator = "&" if parsed.query else "?"
    blocks = []
    with archive.open("rb") as stream:
        while block := stream.read(BLOCK_SIZE):
            if len(blocks) >= 50000:
                raise StoreError("The upload exceeded the Azure block limit.")
            block_id = base64.b64encode(f"teleark-{len(blocks):08d}".encode()).decode()
            journal.record("uploading", uploaded_blocks=len(blocks), archive_bytes=archive.stat().st_size)
            request(transport, "UploadPackageBlock", "PUT", upload_uri + separator + urlencode({"comp": "block", "blockid": block_id}),
                    headers, block, "application/octet-stream")
            blocks.append(block_id)
    body = ("<?xml version=\"1.0\" encoding=\"utf-8\"?><BlockList>" + "".join(f"<Latest>{block}</Latest>" for block in blocks) + "</BlockList>").encode()
    journal.record("finalizing-upload", uploaded_blocks=len(blocks))
    request(transport, "CommitPackageUpload", "PUT", upload_uri + separator + "comp=blocklist", headers, body, "application/xml; charset=utf-8")


def poll(transport, uri: str, headers: dict, journal: Journal, timeout: int, clock, sleep) -> str:
    deadline = clock() + timeout
    while clock() < deadline:
        value = json_response(request(transport, "PollSubmissionStatus", "GET", uri + "/status", headers))
        status = value.get("status")
        if status not in ACCEPTED | FAILED | WAITING:
            raise StoreError("Partner Center returned an unknown submission status.")
        journal.record("waiting-for-commit", status=status)
        if status in FAILED:
            raise StoreError(f"Partner Center rejected the submission ({status}). Inspect Partner Center before retrying.")
        if status in ACCEPTED:
            journal.record("accepted", status=status)
            return status
        sleep(min(10, max(0, deadline - clock())))
    raise StoreError("Commit status is still pending. Retry the same release to monitor its existing submission; do not create another draft.")


def submit(package: Path, journal: Journal, *, transport=None, environment=None, timeout=900, clock=time.monotonic, sleep=time.sleep) -> str:
    transport = transport or HttpTransport()
    environment = os.environ if environment is None else environment
    for name in ("AZURE_AD_TENANT_ID", "AZURE_AD_APPLICATION_CLIENT_ID", "AZURE_AD_APPLICATION_SECRET", "TELEARK_MSSTORE_PRODUCT_ID"):
        if not environment.get(name, "").strip():
            raise StoreError(f"Required Store setting {name} is missing.")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*\.msix", package.name):
        raise StoreError("The package filename must be a safe .msix filename.")
    digest = journal.data["package_sha256"]
    with tempfile.TemporaryDirectory(prefix="teleark-store-") as temp:
        archive = Path(temp) / "upload.zip"
        journal.record("preparing-archive")
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED, allowZip64=True) as zip_file:
            zip_file.write(package, arcname=package.name)
        journal.record("authenticating")
        secret = environment.pop("AZURE_AD_APPLICATION_SECRET")
        try:
            body = urlencode({"grant_type": "client_credentials", "client_id": environment["AZURE_AD_APPLICATION_CLIENT_ID"],
                              "client_secret": secret, "resource": "https://manage.devcenter.microsoft.com"}).encode()
            token = json_response(request(transport, "TokenRequest", "POST",
                f"https://login.microsoftonline.com/{quote(environment['AZURE_AD_TENANT_ID'], safe='')}/oauth2/token",
                body=body, content_type="application/x-www-form-urlencoded; charset=utf-8")).get("access_token")
        finally:
            secret = body = None
        if not isinstance(token, str) or not token or "\r" in token or "\n" in token:
            raise StoreError("Microsoft identity did not return a valid access token.")
        headers = {"Authorization": f"Bearer {token}"}
        app_uri = f"{API}/{quote(environment['TELEARK_MSSTORE_PRODUCT_ID'], safe='')}"
        journal.record("checking-existing-submission")
        app = json_response(request(transport, "GetApplication", "GET", app_uri, headers))
        pending = app.get("pendingApplicationSubmission")
        if pending:
            identifier = submission_id(pending)
            draft = json_response(request(transport, "GetSubmission", "GET", f"{app_uri}/submissions/{identifier}", headers))
            marker = f"TeleArk CI package SHA256: {digest}"
            if marker not in str(draft.get("notesForCertification") or "").splitlines():
                raise StoreError("A different Partner Center draft is active. Finish or remove that draft in Partner Center before retrying.")
        else:
            if not app.get("lastPublishedApplicationSubmission"):
                raise StoreError("The MSIX API requires an earlier completed Partner Center submission with age ratings. Complete the first submission in Partner Center.")
            journal.record("creating-draft")
            draft = json_response(request(transport, "CreateSubmission", "POST", app_uri + "/submissions", headers))
            identifier = submission_id(draft)
        journal.record("draft-ready", submission_id=identifier)
        uri = f"{app_uri}/submissions/{identifier}"
        status = draft.get("status", "PendingCommit")
        if status in ACCEPTED | {"CommitStarted"}:
            return poll(transport, uri, headers, journal, timeout, clock, sleep)
        if status != "PendingCommit":
            raise StoreError("The existing submission cannot be updated in its current state. Inspect Partner Center before retrying.")
        journal.record("updating-draft")
        payload = update_payload(draft, package.name, digest)
        response = json_response(request(transport, "UpdateSubmission", "PUT", uri, headers, json.dumps(payload).encode(), "application/json; charset=utf-8"))
        # The PUT response carries the current SAS URL; prefer it over the create response.
        upload_uri = response.get("fileUploadUrl") or draft.get("fileUploadUrl")
        if not isinstance(upload_uri, str):
            raise StoreError("Partner Center did not provide an upload destination.")
        upload_archive(transport, upload_uri, archive, journal)
        journal.record("committing")
        request(transport, "CommitSubmission", "POST", uri + "/commit", headers)
        return poll(transport, uri, headers, journal, timeout, clock, sleep)


def run_tool(arguments: list[str]) -> str:
    result = subprocess.run(arguments, capture_output=True, text=True, check=False)
    if result.returncode:
        raise StoreError("Release retrieval or tag ancestry verification failed. Check the published release, main branch and GitHub access.")
    return result.stdout


def verify_package(directory: Path, tag: str, environment=None) -> Path:
    environment = os.environ if environment is None else environment
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise StoreError("Choose a numeric vX.Y.Z release tag.")
    package = directory / f"TeleArk-{tag[1:]}-windows-x86_64.msix"
    expected_files = {package.name, "SHA256SUMS"}
    if {item.name for item in directory.iterdir()} != expected_files:
        raise StoreError("Release download must contain exactly the MSIX and SHA256SUMS.")
    entries = {}
    for line in (directory / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([a-fA-F0-9]{64})  ([A-Za-z0-9][A-Za-z0-9._-]*)", line)
        if not match or match[2] in entries:
            raise StoreError("The release checksum manifest is invalid or contains duplicate entries.")
        entries[match[2]] = match[1].lower()
    if entries.get(package.name) != sha256(package):
        raise StoreError("The downloaded MSIX failed release SHA256 verification.")
    try:
        with zipfile.ZipFile(package) as archive:
            info = archive.getinfo("AppxManifest.xml")
            if info.file_size > 64 * 1024:
                raise ValueError()
            identity = ET.fromstring(archive.read(info)).find("{http://schemas.microsoft.com/appx/manifest/foundation/windows10}Identity")
            major, minor, patch = map(int, tag[1:].split("."))
            expected = {"Name": environment["TELEARK_MSIX_IDENTITY_NAME"], "Publisher": environment["TELEARK_MSIX_PUBLISHER"],
                        "Version": f"{major + 1}.{minor}.{patch}.0", "ProcessorArchitecture": "x64"}
            if identity is None or any(identity.get(key) != value for key, value in expected.items()):
                raise StoreError("The downloaded MSIX identity, publisher, architecture or version does not match this Store release.")
    except (KeyError, ValueError, zipfile.BadZipFile, ET.ParseError):
        raise StoreError("The downloaded MSIX manifest is missing or invalid; configure the exact Partner Center identity.") from None
    return package


def prepare_release(directory: Path, tag: str) -> Path:
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise StoreError("Choose a numeric vX.Y.Z release tag.")
    emit("validating-release")
    commit = run_tool(["git", "rev-parse", f"refs/tags/{tag}^{{commit}}"]).strip()
    run_tool(["git", "merge-base", "--is-ancestor", commit, "origin/main"])
    release = json_response(run_tool(["gh", "release", "view", tag, "--json", "tagName,isDraft"]).encode())
    if release.get("tagName") != tag or release.get("isDraft") is not False:
        raise StoreError("Store submission requires a published GitHub Release with the matching tag.")
    directory.mkdir(parents=True, exist_ok=True)
    emit("downloading-release-package")
    run_tool(["gh", "release", "download", tag, "--pattern", f"TeleArk-{tag[1:]}-windows-x86_64.msix", "--pattern", "SHA256SUMS", "--dir", str(directory)])
    emit("verifying-release-package")
    return verify_package(directory, tag)


def main() -> int:
    parser = argparse.ArgumentParser(description="Submit a verified MSIX release to Microsoft Store.")
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--release-tag", required=True)
    prepare.add_argument("--directory", type=Path, required=True)
    upload = commands.add_parser("submit")
    upload.add_argument("--package-path", type=Path, required=True)
    upload.add_argument("--receipt-path", type=Path, default=Path("store-result/submission.json"))
    args = parser.parse_args()
    journal = None
    try:
        if args.command == "prepare":
            prepare_release(args.directory, args.release_tag)
        else:
            journal = Journal(args.receipt_path, args.package_path)
            status = submit(args.package_path, journal)
            print(f"Microsoft Store accepted the submission ({status}); Microsoft controls certification and publication.", flush=True)
        return 0
    except KeyboardInterrupt:
        if journal:
            journal.record("interrupted")
        print("Microsoft Store submission interrupted. Check the receipt and Partner Center before retrying.", file=sys.stderr)
        return 130
    except Exception as failure:
        if journal:
            journal.record("failed")
        message = str(failure) if isinstance(failure, StoreError) else "Local submission preparation failed. Check file access and tool availability."
        print(f"Microsoft Store: {message}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
