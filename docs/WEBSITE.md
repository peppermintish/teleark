# Public website

The [TeleArk website](https://peppermintish.github.io/teleark/) is a dependency-free static site published by [the Pages workflow](../.github/workflows/pages.yml). The [README](../README.md) links to its downloads and macOS source-build guide; the website links back to the README and source documentation.

## Preview and validate

Use Python 3.11 or later from the repository root. These commands use public, checked-in release metadata and need no credentials:

```sh
python -m unittest discover -s scripts/tests -p test_site.py -v
python scripts/build-site.py
python -m http.server 8080 --bind 127.0.0.1 --directory target/site-preview
```

Open `http://127.0.0.1:8080/`. Review English/light layouts at 900×600, full-screen desktop and 375-pixel mobile widths, including landscape, larger text, keyboard focus, disclosure controls and the macOS guide. The preview image is captured from `--preview-ui` with synthetic files; it contains no Telegram session or personal documents. Logo artwork comes from TeleArk's existing repository assets. Custom SVG labels use text and simple geometric symbols.

`site/index.html` is a template. `scripts/build-site.py` substitutes validated release tags, filenames and direct download URLs. It copies only the explicit public site files and assets into `target/site-preview`; raw metadata, local environment files and other repository files are excluded. No browser JavaScript, external fonts, analytics or cookies are needed. The generated page keeps working when JavaScript is disabled.

## Downloads and publication

Windows buttons always link to the official [Microsoft Store product](https://apps.microsoft.com/detail/9NJD0FKQVR9B). Linux/macOS downloads use the exact AppImage, Debian, archive, universal installer and standalone executable names required by the release contract. The site includes the release's checksum file and explains the absence of Apple notarization. Source-build instructions compile a host-native executable and do not require the project's private packaging identity.

Local previews default to the stable snapshot in `site/release.json`. To preview currently published downloads instead:

```sh
python scripts/build-site.py --refresh-release
```

Production builds fetch GitHub's latest stable public release with a bounded request, then validate the tag, repository and every required artifact URL. Invalid, missing, draft, prerelease or noncanonical data fails the build before publication; the previously deployed site stays available. No token is used. Update the offline snapshot when appropriate for future local previews; it is not the production source of truth.

In **Repository Settings → Pages → Build and deployment**, select **GitHub Actions**. The workflow tests and deploys site changes on `main` and supports manual dispatch. It rebuilds after externally published releases and completion of tagged **TeleArk CI/CD** runs. The latter handles releases created by `GITHUB_TOKEN`, whose release events do not start another workflow. Tagged CI completion fetches the latest actually published release even if Store delivery subsequently failed. Both paths check out the current `main` website rather than an older release tag; they never execute artifacts or code from the triggering run. PRs test and build the offline site without Pages permissions or deployment. Production deployments queue instead of interrupting an in-progress deployment. The `github-pages` environment receives the deployed URL.

A manual **Publish TeleArk website** run on `main` also refreshes all download links without rebuilding the desktop app. This website has no application persistence or codec changes. Website task checkpoints use annotated documentation tags, separate from application `vX.Y.Z` release tags and Microsoft Store delivery.
