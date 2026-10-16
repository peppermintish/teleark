"""Website/download contract checks: synthetic metadata, no credentials or live calls."""

import copy
from html.parser import HTMLParser
import importlib.util
import json
from pathlib import Path
import re
import tempfile
import unittest
from unittest import mock
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("build_site", ROOT / "scripts/build-site.py")
site = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(site)


class Document(HTMLParser):
    def __init__(self, source):
        super().__init__()
        self.ids = []
        self.links = []
        self.images = []
        self.scripts = []
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.ids.append(attrs["id"])
        if tag in ("a", "link"):
            self.links.append(attrs["href"])
        if tag == "img":
            self.images.append(attrs)
        if tag == "script":
            self.scripts.append(attrs)


class SiteTests(unittest.TestCase):
    def setUp(self):
        self.release = json.loads((ROOT / "site/release.json").read_text(encoding="utf-8"))

    def test_future_release_updates_every_download_and_build_command(self):
        future = copy.deepcopy(self.release)
        old = future["tag_name"][1:]
        future["tag_name"] = "v12.34.56"
        future["html_url"] = f"{site.REPOSITORY}/releases/tag/v12.34.56"
        for asset in future["assets"]:
            asset["name"] = asset["name"].replace(old, "12.34.56")
            asset["browser_download_url"] = asset["browser_download_url"].replace(old, "12.34.56")
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            self.assertEqual(site.build(future, output), "v12.34.56")
            source = (output / "index.html").read_text(encoding="utf-8")
            self.assertNotIn("{{", source)
            self.assertIn("git switch --detach v12.34.56", source)
            self.assertNotIn(old, source)
            for key, value in site.release_values(future).items():
                if key.endswith("_url"):
                    self.assertIn(value, source)

    def test_incomplete_or_untrusted_release_preserves_previous_page(self):
        for mutation in ("missing", "wrong-host", "duplicate", "draft", "prerelease", "wrong-repository", "invalid-tag"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                invalid = copy.deepcopy(self.release)
                if mutation == "missing":
                    invalid["assets"] = []
                elif mutation == "wrong-host":
                    download = next(asset for asset in invalid["assets"] if asset["name"].endswith(".AppImage"))
                    download["browser_download_url"] = "https://example.org/download"
                elif mutation == "duplicate":
                    invalid["assets"].append(copy.deepcopy(invalid["assets"][0]))
                elif mutation in ("draft", "prerelease"):
                    invalid[mutation] = True
                elif mutation == "wrong-repository":
                    invalid["html_url"] = "https://github.com/another/project/releases/tag/v1.0.0"
                else:
                    invalid["tag_name"] = 'v1.0.0\" onclick=\"alert(1)'
                output = Path(directory)
                (output / "index.html").write_text("previous deployment", encoding="utf-8")
                with self.assertRaises(ValueError):
                    site.build(invalid, output)
                self.assertEqual((output / "index.html").read_text(encoding="utf-8"), "previous deployment")

    def test_fetch_is_bounded_and_times_out(self):
        response = mock.MagicMock()
        response.__enter__.return_value = response
        response.read.return_value = b"x" * (site.MAX_METADATA_BYTES + 1)
        with mock.patch.object(site.urllib.request, "urlopen", return_value=response) as remote:
            with self.assertRaises(ValueError):
                site.fetch_release()
            self.assertEqual(remote.call_args.kwargs["timeout"], 20)
            response.read.assert_called_once_with(site.MAX_METADATA_BYTES + 1)

    def test_network_failure_is_reported_without_success_or_publication(self):
        with mock.patch.object(site, "fetch_release", side_effect=OSError("synthetic offline")), \
             mock.patch.object(site, "build") as build, \
             mock.patch.object(site.sys, "argv", ["build-site.py", "--refresh-release"]):
            self.assertEqual(site.main(), 1)
            build.assert_not_called()

    def test_unknown_template_values_fail_and_values_are_escaped(self):
        with self.assertRaises(ValueError):
            site.render("{{missing}}", {})
        self.assertEqual(site.render("{{label}}", {"label": '<a href="unsafe">'}), "&lt;a href=&quot;unsafe&quot;&gt;")

    def test_output_cannot_overwrite_source(self):
        for output in (ROOT, ROOT.parent, site.SOURCE, site.SOURCE / "generated"):
            with self.subTest(output=output), self.assertRaises(ValueError):
                site.build(self.release, output)

    def test_public_artifact_and_local_links(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            site.build(self.release, output)
            self.assertEqual({item.name for item in output.iterdir()}, {"index.html", "styles.css", "404.html", "robots.txt", "sitemap.xml", "assets", ".nojekyll"})
            doc = Document((output / "index.html").read_text(encoding="utf-8"))
            self.assertEqual(len(doc.ids), len(set(doc.ids)))
            self.assertFalse(doc.scripts)
            for link in doc.links:
                parsed = urlsplit(link)
                if parsed.scheme:
                    self.assertEqual(parsed.scheme, "https")
                elif parsed.fragment:
                    self.assertIn(parsed.fragment, doc.ids)
                elif parsed.path not in ("", "./"):
                    self.assertTrue((output / parsed.path).is_file(), link)
            for image in doc.images:
                self.assertIn("alt", image)
                self.assertIn("width", image)
                self.assertIn("height", image)
                self.assertTrue((output / image["src"]).is_file())

    def test_readme_assets_and_document_links_exist(self):
        source = (ROOT / "README.md").read_text(encoding="utf-8")
        links = re.findall(r"\]\(([^)]+)\)|(?:src|href)=\"([^\"]+)\"", source)
        for markdown, raw in links:
            parsed = urlsplit(markdown or raw)
            if not parsed.scheme and parsed.path:
                self.assertTrue((ROOT / parsed.path).exists(), parsed.path)


if __name__ == "__main__":
    unittest.main()
