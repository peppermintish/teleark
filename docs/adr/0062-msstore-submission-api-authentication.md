# ADR 0062: Microsoft Store Submission API for MSIX releases

Status: Superseded in implementation and release ordering by [ADR 0063](0063-independent-msix-store-delivery.md) — 2026-09-30. The MSIX authentication endpoint and existing credential names remain applicable. Previously superseded the Store mechanism in ADRs 0060 and 0061.

## Context

The Microsoft Store CLI accepts its client secret as a command option, which would place the value in process arguments. The legacy Microsoft Store Submission API supports MSIX and accepts Entra client-credentials authentication through an HTTPS form request. Its submission flow requires the writable draft JSON, a ZIP archive containing the package, upload to a returned Azure Blob SAS URL, commit, and status polling. The newer `api.store.microsoft.com` submission API is for MSI/EXE packages and does not replace the legacy MSIX API.

Microsoft's legacy API requires an app with at least one completed submission and a configured age rating. The first upload remains manual through Partner Center. Certification remains controlled by Microsoft.

## Decision

For version-tag releases, use the existing `AZURE_AD_TENANT_ID`, `AZURE_AD_APPLICATION_CLIENT_ID`, and `AZURE_AD_APPLICATION_SECRET` secrets plus `TELEARK_MSSTORE_PRODUCT_ID`. Validate these settings only in the `store-submission` job after the verified Windows package is ready. The job uses the `store-submission` GitHub Environment, so the preflight, quality, test and package jobs never receive Store credentials and can run in parallel. The validation helper reports missing settings without values. Read the client secret from the scoped process environment inside the submission routine. Send it only in the OAuth form body over HTTPS, then clear the local value, request body reference and process environment value as soon as token acquisition completes. Never place the secret in arguments, files, logs or failure output. The Store job does not request `id-token: write`.

Prepare a temporary ZIP containing the verified `.msix` before creating a draft so the upload URL can be used promptly. Use the API's returned writable submission record as the basis of the update, replacing only the package metadata required for upload. Stream the ZIP in bounded Azure Blob blocks to an HTTPS `*.blob.core.windows.net` SAS URL, without forwarding the bearer token or logging the URL. Commit once, then poll status; do not retry an ambiguous draft creation or commit automatically. Report safe HTTP/status information and direct operators to inspect Partner Center before retrying.

The legacy MSIX API does not document Seller ID as an input. The existing `SELLER_ID` GitHub secret may remain configured, but release validation and submission do not require or read it. Associate the Entra app registration with Partner Center and assign it the Manager role.

## Consequences

Partner Center must already contain a completed submission and age rating before automated updates can run. A submission/API failure blocks GitHub Release publication. A failed or ambiguous API call may leave an active draft, which must be inspected in Partner Center before another release attempt. This workflow does not submit the initial app, verify live certification, or publish to the Store independently of Microsoft's review.

## Verification

Synthetic HTTP tests verify OAuth form construction, secret and SAS redaction, full submission metadata handling, ZIP contents, bounded block upload and ordering, commit and polling statuses, ambiguous failures, and environment cleanup. Configuration tests use synthetic values and verify that output omits them. Workflow graph checks verify that Store settings are absent from preflight, are validated in the protected Store job before submission, and that Store submission waits for quality gates and the verified Windows package; release assembly waits for all package jobs and Store submission. No live Store API calls or credentials are used in local tests.

## References

- [Create and manage submissions using Windows Store services](https://learn.microsoft.com/en-us/windows/uwp/monetize/create-and-manage-submissions-using-windows-store-services)
- [Manage app submissions](https://learn.microsoft.com/en-us/windows/uwp/monetize/manage-app-submissions)
- [Update an app submission](https://learn.microsoft.com/en-us/windows/uwp/monetize/update-an-app-submission)
- [Upload MSIX app packages](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/upload-app-packages)
- [Azure Blob Put Block](https://learn.microsoft.com/en-us/rest/api/storageservices/put-block)
- [Azure Blob Put Block List](https://learn.microsoft.com/en-us/rest/api/storageservices/put-block-list)
