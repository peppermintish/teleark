# ADR 0063: Independent, resumable MSIX Store delivery

Status: Accepted — 2026-09-30. Supersedes ADR 0062's PowerShell HTTP client and the requirement that Store submission precede GitHub Release publication. Microsoft's MSIX API authentication endpoint and existing credential names remain applicable.

## Context

Hosted v0.5.12 completed the native tests and packages but failed at `TokenRequest` before any network request: the default PowerShell closure could not resolve the module-private transport. The v0.5.13 fix failed in Linux quality validation with `A safe transport failure omitted its allow-listed diagnostics`, preventing Store submission. The same tests passed locally. The old JSON/XML body constructors also rejected media-type strings containing charset parameters, a defect hidden by mocked transport success responses. A chain of closure-bound mocks did not provide reliable evidence for the production request path, and every Store failure prevented publication of otherwise verified open-source releases.

## Decision

Use a Python standard-library HTTP client with ordinary function/class ownership, without PowerShell module callbacks. Test its full production transport with synthetic HTTP responses over a local socket on every native OS runner. Implement the MSIX Dev Center API described by Microsoft, including the ZIP upload, writable draft fields, `PendingDelete`/`PendingUpload` package states, bounded Azure block upload, commit and status polling. Prefer the current SAS URL from the draft update response. Preserve listing/publication settings; exclude read-only fields and unsupported Pricing Version 2 from package updates. No new runtime or Python package dependency is added to the application.

Publish the checksummed GitHub Release only after all existing quality, native-test and platform-package gates succeed. Run Store delivery afterward through a reusable, independently dispatchable workflow. Verify tag ancestry, release checksum, MSIX manifest identity, publisher, version and architecture before exposing Store credentials. Manual retries execute trusted `main` code and use the existing published release. Keep secrets in the existing `store-submission` Environment and serialize Store mutations across tags/retries.

Read the app before draft creation. A matching package SHA256 marker in certification notes permits safe resumption of an uncommitted draft or monitoring of a committed draft. Preserve unrelated/unmarked drafts and report operator guidance. Never automatically repeat ambiguous create/commit calls or delete/cancel submissions. Preserve the original certification notes when appending the marker.

Record phase acknowledgment before each expensive operation. Receipt schema 1 contains only package name/hash, validated submission ID, allow-listed status, phase and a bounded timestamped timeline. Receipt failures cannot stall submission. Retain receipts on success/failure; do not log OAuth forms/tokens, SAS URLs, remote listing data or raw exception/response text. Report accepted ingestion separately from Microsoft's certification/publication.

## Consequences and validation

Store outages do not remove or block the completed GitHub Release. They still fail the Store stage, which can be retried with the same version. The existing Environment must allow `main` for manual retries as well as version tags, retaining its protection/review policy. Microsoft's first-submission and unsupported-feature restrictions still apply.

Synthetic tests cover the default HTTP request path, form encoding, ZIP/block contents/order and isolation, metadata preservation, safe errors, pending draft ownership, upload resumption, no second commit, cancellation, rejection/unknown status, controlled-clock timeout, bounded receipts, release hash/manifest checks and rejected destinations/redirects. Hosted delivery is required to establish actual Partner Center acceptance.

## Official references

- [MSIX submission authentication and restrictions](https://learn.microsoft.com/en-us/windows/uwp/monetize/create-and-manage-submissions-using-windows-store-services)
- [App submission flow and data resources](https://learn.microsoft.com/en-us/windows/uwp/monetize/manage-app-submissions)
- [Read app and pending submission](https://learn.microsoft.com/en-us/windows/uwp/monetize/get-an-app)
- [Update a submission](https://learn.microsoft.com/en-us/windows/uwp/monetize/update-an-app-submission)
- [Submission statuses](https://learn.microsoft.com/en-us/windows/uwp/monetize/get-status-for-an-app-submission)
- [Azure Put Block](https://learn.microsoft.com/en-us/rest/api/storageservices/put-block) and [Put Block List](https://learn.microsoft.com/en-us/rest/api/storageservices/put-block-list)
- [Separate MSI/EXE API](https://learn.microsoft.com/en-us/windows/apps/publish/store-submission-api)
