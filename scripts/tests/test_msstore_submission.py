"""Synthetic Store contract tests, including the real HTTP client over loopback."""

from contextlib import redirect_stdout
from http.server import BaseHTTPRequestHandler, HTTPServer
import http.client
import io
import json
from pathlib import Path
import socket
import ssl
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import msstore_submission as store

SECRET = "synthetic-secret+/%& never-log"
TOKEN = "synthetic-access-token-never-log"
SAS = "synthetic-sas-never-log"
ENV = {
    "AZURE_AD_TENANT_ID": "synthetic-tenant", "AZURE_AD_APPLICATION_CLIENT_ID": "synthetic-client",
    "AZURE_AD_APPLICATION_SECRET": SECRET, "TELEARK_MSSTORE_PRODUCT_ID": "9SYNTHETIC",
    "TELEARK_MSIX_IDENTITY_NAME": "Test.TeleArk", "TELEARK_MSIX_PUBLISHER": "CN=Synthetic",
}


def draft():
    return {
        "id": "submission-123", "status": "PendingCommit", "friendlyName": "Synthetic submission",
        "fileUploadUrl": f"https://test.blob.core.windows.net/archive?sig={SAS}",
        "pricing": {"priceId": "Free", "trialPeriod": "NotAvailable", "isAdvancedPricingModel": True, "sales": []},
        "listings": {"en-us": {"baseListing": {"title": "Keep my title", "images": [{"fileStatus": "Uploaded"}]}}},
        "visibility": "Public", "targetPublishMode": "Manual", "notesForCertification": "Keep these notes",
        "applicationPackages": [{"fileName": "prior.msix", "fileStatus": "Uploaded", "architecture": "x64"}],
    }


class Remote:
    def __init__(self):
        self.requests = []
        self.current = draft()
        self.pending = None
        self.published = {"id": "previous"}
        self.statuses = ["CommitStarted", "PreProcessing"]
        self.fail_operation = None
        self.failure = None

    def request(self, method, uri, headers, body=None):
        self.requests.append((method, uri, headers.copy(), body))
        path = urlsplit(uri).path
        operation = "TokenRequest" if path.endswith("/token") else ""
        if path.endswith("/commit"):
            operation = "CommitSubmission"
        if method == "POST" and path.endswith("/submissions"):
            operation = "CreateSubmission"
        if "comp=block&" in uri:
            operation = "UploadPackageBlock"
        if self.fail_operation == operation:
            if isinstance(self.failure, Exception):
                raise self.failure
            return self.failure, f"{SECRET} {TOKEN} {SAS}".encode()
        if path.endswith("/token"):
            return 200, json.dumps({"access_token": TOKEN}).encode()
        if path.endswith("/applications/9SYNTHETIC"):
            return 200, json.dumps({"lastPublishedApplicationSubmission": self.published, "pendingApplicationSubmission": self.pending}).encode()
        if method == "POST" and path.endswith("/submissions"):
            return 201, json.dumps(self.current).encode()
        if method == "GET" and path.endswith("/submission-123"):
            return 200, json.dumps(self.current).encode()
        if method == "PUT" and path.endswith("/submission-123"):
            self.updated = json.loads(body)
            return 200, json.dumps({"fileUploadUrl": f"https://test.blob.core.windows.net/current-archive?sig={SAS}"}).encode()
        if method == "PUT" and "comp=" in uri:
            return 201, b""
        if path.endswith("/commit"):
            return 200, b"{}"
        if path.endswith("/status"):
            status = self.statuses.pop(0) if len(self.statuses) > 1 else self.statuses[0]
            return 200, json.dumps({"status": status}).encode()
        raise AssertionError("Unexpected synthetic HTTP request")


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="teleark-store-test-")
        self.directory = Path(self.temp.name)
        self.package = self.directory / "TeleArk-0.5.14-windows-x86_64.msix"
        self.package.write_bytes(b"synthetic-package-contents")
        self.journal = store.Journal(self.directory / "receipt.json", self.package)
        self.remote = Remote()
        self.env = ENV.copy()
        self.output = io.StringIO()
        self.time = 0
        self.addCleanup(self.temp.cleanup)

    def sleep(self, duration):
        self.time += duration

    def submit(self, transport=None, timeout=30):
        with redirect_stdout(self.output):
            return store.submit(self.package, self.journal, transport=transport or self.remote,
                                environment=self.env, timeout=timeout, clock=lambda: self.time, sleep=self.sleep)

    def assert_redacted(self, value):
        for sensitive in (SECRET, TOKEN, SAS, ENV["AZURE_AD_TENANT_ID"], ENV["AZURE_AD_APPLICATION_CLIENT_ID"]):
            self.assertNotIn(sensitive, value)

    def test_full_contract_preserves_listing_and_streams_only_zip(self):
        with patch.object(store, "BLOCK_SIZE", 64):
            self.assertEqual(self.submit(), "PreProcessing")
        self.assertNotIn("AZURE_AD_APPLICATION_SECRET", self.env)
        method, token_uri, headers, body = self.remote.requests[0]
        self.assertEqual(method, "POST")
        self.assertEqual(token_uri, "https://login.microsoftonline.com/synthetic-tenant/oauth2/token")
        form = parse_qs(body.decode())
        self.assertEqual(form["client_secret"], [SECRET])
        self.assertEqual(form["resource"], ["https://manage.devcenter.microsoft.com"])
        self.assertNotIn("Authorization", headers)
        self.assertEqual(self.remote.updated["listings"], draft()["listings"])
        self.assertEqual(self.remote.updated["targetPublishMode"], "Manual")
        self.assertEqual(self.remote.updated["pricing"], {"priceId": "Free", "trialPeriod": "NotAvailable"})
        self.assertNotIn("friendlyName", self.remote.updated)
        self.assertNotIn("fileUploadUrl", self.remote.updated)
        self.assertEqual(self.remote.updated["applicationPackages"][0]["fileStatus"], "PendingDelete")
        self.assertEqual(self.remote.updated["applicationPackages"][1]["fileName"], self.package.name)
        self.assertEqual(self.remote.updated["applicationPackages"][1]["fileStatus"], "PendingUpload")
        uploads = [item for item in self.remote.requests if "comp=block&" in item[1]]
        self.assertGreater(len(uploads), 1)
        for _, uri, headers, block in uploads:
            self.assertIn("/current-archive?", uri)
            self.assertNotIn("Authorization", headers)
            self.assertLessEqual(len(block), 64)
            self.assertEqual(headers["x-ms-version"], "2023-11-03")
        with zipfile.ZipFile(io.BytesIO(b"".join(item[3] for item in uploads))) as archive:
            self.assertEqual(archive.namelist(), [self.package.name])
            self.assertEqual(archive.read(self.package.name), self.package.read_bytes())
        last = [item for item in self.remote.requests if "comp=blocklist" in item[1]][0]
        ids = [parse_qs(urlsplit(item[1]).query)["blockid"][0] for item in uploads]
        self.assertIn("".join(f"<Latest>{value}</Latest>" for value in ids), last[3].decode())
        self.assertEqual(self.journal.data["phase"], "accepted")
        self.assertEqual(self.time, 10)
        self.assert_redacted(self.output.getvalue() + self.journal.path.read_text())

    def test_default_transport_entire_flow_uses_real_http_framing(self):
        remote = self.remote

        class Handler(BaseHTTPRequestHandler):
            def handle_request(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", 0))) or None
                status, content = remote.request(self.command, "https://loopback" + self.path, dict(self.headers), body)
                self.send_response(status)
                self.send_header("Content-Length", str(len(content)))
                self.end_headers()
                self.wfile.write(content)

            do_GET = do_POST = do_PUT = handle_request

            def log_message(self, *args):
                pass

        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            def connection(*args, **kwargs):
                return http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=2)

            with patch.object(http.client, "HTTPSConnection", side_effect=connection):
                self.assertEqual(self.submit(transport=store.HttpTransport()), "PreProcessing")
            self.assertTrue(any(item[2].get("Authorization") == f"Bearer {TOKEN}" for item in remote.requests))
            self.assertTrue(any(item[2].get("Content-Type") == "application/xml; charset=utf-8" for item in remote.requests))
            self.assertTrue(any(item[2].get("Content-Type") == "application/json; charset=utf-8" for item in remote.requests))
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=3)
            self.assertFalse(thread.is_alive())

    def test_phase_feedback_precedes_authentication(self):
        remote = self.remote

        class ObservedRemote:
            def request(inner, *args):
                if args[1].endswith("/token"):
                    self.assertEqual(self.journal.data["phase"], "authenticating")
                    self.assertIn("authenticating", self.output.getvalue())
                    self.assertNotIn("AZURE_AD_APPLICATION_SECRET", self.env)
                return remote.request(*args)

        self.submit(ObservedRemote())

    def test_ambiguous_mutations_are_never_retried(self):
        for operation in ("CreateSubmission", "CommitSubmission", "UploadPackageBlock"):
            with self.subTest(operation=operation):
                self.remote = Remote()
                self.env = ENV.copy()
                self.remote.fail_operation = operation
                self.remote.failure = TimeoutError(f"{SECRET} {TOKEN} {SAS}")
                with self.assertRaises(store.StoreError) as caught:
                    self.submit()
                self.assertIn(operation, str(caught.exception))
                self.assertIn("transport=timeout", str(caught.exception))
                self.assertIsNone(caught.exception.__cause__)
                self.assert_redacted(str(caught.exception) + self.output.getvalue())
                self.assertEqual(sum("/commit" in item[1] for item in self.remote.requests), int(operation == "CommitSubmission"))

    def test_token_failures_classified_without_disclosing_response_or_exception(self):
        for failure, expected in ((401, "HTTP 401"), (403, "HTTP 403"), (409, "HTTP 409"),
                                  (socket.gaierror(1, SECRET), "transport=dns"), (ssl.SSLError(SECRET), "transport=tls")):
            with self.subTest(expected=expected):
                self.remote = Remote()
                self.env = ENV.copy()
                self.remote.fail_operation = "TokenRequest"
                self.remote.failure = failure
                with self.assertRaises(store.StoreError) as caught:
                    self.submit()
                self.assertIn(expected, str(caught.exception))
                self.assert_redacted(str(caught.exception))
                self.assertEqual(len(self.remote.requests), 1)
                self.assertNotIn("AZURE_AD_APPLICATION_SECRET", self.env)

    def test_existing_unrelated_draft_is_not_modified_or_deleted(self):
        self.remote.pending = {"id": "submission-123"}
        with self.assertRaisesRegex(store.StoreError, "different Partner Center draft"):
            self.submit()
        self.assertFalse(any(item[0] in {"PUT", "DELETE"} for item in self.remote.requests))
        self.assertFalse(any(item[1].endswith("/submissions") for item in self.remote.requests))

    def test_committed_same_package_resumes_polling_without_upload_or_commit(self):
        self.remote.pending = {"id": "submission-123"}
        self.remote.current["notesForCertification"] = f"TeleArk CI package SHA256: {store.sha256(self.package)}"
        self.remote.current["status"] = "CommitStarted"
        self.assertEqual(self.submit(), "PreProcessing")
        self.assertFalse(any(item[0] == "PUT" or item[1].endswith("/commit") for item in self.remote.requests))

    def test_uncommitted_same_package_can_resume_safe_upload(self):
        self.remote.pending = {"id": "submission-123"}
        self.remote.current["notesForCertification"] = f"TeleArk CI package SHA256: {store.sha256(self.package)}"
        self.assertEqual(self.submit(), "PreProcessing")
        self.assertFalse(any(item[0] == "POST" and item[1].endswith("/submissions") for item in self.remote.requests))

    def test_first_submission_requires_partner_center(self):
        self.remote.published = None
        with self.assertRaisesRegex(store.StoreError, "first submission"):
            self.submit()
        self.assertFalse(any(item[1].endswith("/submissions") for item in self.remote.requests))

    def test_failure_unknown_and_timeout_are_not_success(self):
        for status, expected in (("CommitFailed", "rejected"), ("CertificationFailed", "rejected"),
                                  ("invented-status", "unknown"), ("CommitStarted", "pending")):
            with self.subTest(status=status):
                self.env = ENV.copy()
                self.remote = Remote()
                self.remote.statuses = [status]
                self.time = 0
                with self.assertRaisesRegex(store.StoreError, expected):
                    self.submit(timeout=2)

    def test_untrusted_upload_url_never_receives_credentials_or_package(self):
        self.remote.current["fileUploadUrl"] = "https://evil.invalid/upload"
        original = self.remote.request

        def transport(method, uri, headers, body=None):
            if method == "PUT" and uri.endswith("/submission-123"):
                return 200, b"{}"
            return original(method, uri, headers, body)

        with patch.object(self.remote, "request", side_effect=transport):
            with self.assertRaisesRegex(store.StoreError, "invalid Azure Blob"):
                self.submit()
        self.assertFalse(any("evil.invalid" in item[1] for item in self.remote.requests))

    def test_pricing_version_two_is_omitted_from_package_update(self):
        self.remote.current["pricing"]["priceId"] = "Unknown"
        self.submit()
        self.assertNotIn("pricing", self.remote.updated)
        self.assertEqual(self.remote.updated["listings"], draft()["listings"])

    def test_cancellation_leaves_no_commit_and_archive_is_removed(self):
        original_temporary_directory = tempfile.TemporaryDirectory
        archive_directories = []

        def temporary_directory(*args, **kwargs):
            result = original_temporary_directory(*args, **kwargs)
            archive_directories.append(Path(result.name))
            return result

        with patch.object(store.tempfile, "TemporaryDirectory", side_effect=temporary_directory):
            with patch.object(self.remote, "request", side_effect=KeyboardInterrupt):
                with self.assertRaises(KeyboardInterrupt):
                    self.submit()
        self.assertNotIn("AZURE_AD_APPLICATION_SECRET", self.env)
        self.assertFalse(any(item[1].endswith("/commit") for item in self.remote.requests))
        self.assertEqual(len(archive_directories), 1)
        self.assertFalse(archive_directories[0].exists())

    def test_receipt_retention_is_bounded_and_disclosed(self):
        with redirect_stdout(self.output):
            for _ in range(80):
                self.journal.record("waiting-for-commit", status="CommitStarted")
        self.assertEqual(len(self.journal.data["events"]), 64)
        self.assertEqual(self.journal.data["omitted_events"], 16)
        self.assertEqual(json.loads(self.journal.path.read_text())["schema_version"], 1)

    def test_release_verification_covers_hash_identity_version_and_extra_files(self):
        manifest = '<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"><Identity Name="Test.TeleArk" Publisher="CN=Synthetic" Version="1.5.14.0" ProcessorArchitecture="x64"/></Package>'
        with zipfile.ZipFile(self.package, "w") as archive:
            archive.writestr("AppxManifest.xml", manifest)
        checksums = self.directory / "SHA256SUMS"
        checksums.write_text(f"{store.sha256(self.package)}  {self.package.name}\n", encoding="utf-8")
        self.assertEqual(store.verify_package(self.directory, "v0.5.14", ENV), self.package)
        with zipfile.ZipFile(self.package, "w") as archive:
            archive.writestr("AppxManifest.xml", manifest.replace('Version="1.5.14.0"', 'Version="1.5.13.0"'))
        checksums.write_text(f"{store.sha256(self.package)}  {self.package.name}\n", encoding="utf-8")
        with self.assertRaisesRegex(store.StoreError, "does not match"):
            store.verify_package(self.directory, "v0.5.14", ENV)
        for setting in ("TELEARK_MSIX_IDENTITY_NAME", "TELEARK_MSIX_PUBLISHER"):
            with self.subTest(setting=setting):
                env = ENV | {setting: "mismatched"}
                with self.assertRaisesRegex(store.StoreError, "does not match"):
                    store.verify_package(self.directory, "v0.5.14", env)
        checksums.write_text(f"{'0' * 64}  {self.package.name}\n", encoding="utf-8")
        with self.assertRaisesRegex(store.StoreError, "SHA256"):
            store.verify_package(self.directory, "v0.5.14", ENV)
        (self.directory / "extra.msix").touch()
        with self.assertRaisesRegex(store.StoreError, "exactly"):
            store.verify_package(self.directory, "v0.5.14", ENV)

    def test_untrusted_or_unpublished_release_is_rejected_before_download(self):
        with patch.object(store, "run_tool") as tool:
            for tag in ("main", "v1.2.3/other", "v1.2.3; command", "v1.2.3\n"):
                with self.subTest(tag=tag), self.assertRaises(store.StoreError):
                    store.prepare_release(self.directory, tag)
            tool.assert_not_called()
        with redirect_stdout(self.output):
            with patch.object(store, "run_tool", side_effect=["a" * 40, store.StoreError("Tag is outside main")]) as tool:
                with self.assertRaises(store.StoreError):
                    store.prepare_release(self.directory, "v0.5.14")
                self.assertFalse(any("download" in call.args[0] for call in tool.call_args_list))
            with patch.object(store, "run_tool", side_effect=["a" * 40, "", '{"tagName":"v0.5.14","isDraft":true}']) as tool:
                with self.assertRaisesRegex(store.StoreError, "published GitHub Release"):
                    store.prepare_release(self.directory, "v0.5.14")
                self.assertFalse(any("download" in call.args[0] for call in tool.call_args_list))

    def test_receipt_io_failure_cannot_stop_delivery(self):
        with patch.object(Path, "replace", side_effect=OSError(SECRET)):
            self.assertEqual(self.submit(), "PreProcessing")
        self.assertIn("diagnostic receipt could not be saved", self.output.getvalue())
        self.assert_redacted(self.output.getvalue())

    def test_redirect_is_failure_and_is_not_followed(self):
        class Redirect:
            def request(inner, *args):
                return 302, b"https://evil.invalid/" + SECRET.encode()

        with self.assertRaisesRegex(store.StoreError, "HTTP 302") as caught:
            self.submit(Redirect())
        self.assert_redacted(str(caught.exception))


if __name__ == "__main__":
    unittest.main()
