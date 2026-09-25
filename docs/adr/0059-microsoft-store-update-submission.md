# ADR 0059: Microsoft Store update submission from GitHub Actions

Status: Accepted — 2026-09-25. This supersedes ADR 0058's manual-only submission path for updates. The first Store submission remains manual.

## Decision

After every release-tag package matrix succeeds, optionally submit the verified raw `.msix` to Partner Center through Microsoft's `microsoft-store-apppublisher` GitHub Action and Store Developer CLI. Enable the step only when the four authentication secrets (`AZURE_AD_TENANT_ID`, `AZURE_AD_APPLICATION_CLIENT_ID`, `AZURE_AD_APPLICATION_SECRET`, `SELLER_ID`) and the public `TELEARK_MSSTORE_PRODUCT_ID` repository variable are all present. Any partial configuration fails early. The existing three MSIX manifest identity variables continue to control package creation independently. Pull requests and manual package previews never submit.

The Microsoft Entra app must be associated with the Partner Center account and granted the Manager role. Microsoft's current GitHub Actions update path requires a free app already published and live in the Store. Create and certify the initial submission manually, including the age-rating questionnaire. The automated step submits a package for certification; it does not bypass Store review or immediately publish a live update.

## Security and operations

Keep Tenant ID, Client ID, client secret and Seller ID in GitHub Actions secrets. Keep the Store Product ID in a repository variable; it identifies the public product and is not an authentication credential. The submission job runs only on version tags after all platforms' package checks succeed. It receives the secrets only in the submission job, never in pull-request or package-preview jobs. Rotate the client secret before its Entra expiration and update the GitHub secret at the same time.

## Consequences

With the configuration complete, pushing a matching `vX.Y.Z` tag starts Store submission alongside GitHub Release publication. Partner Center acceptance, certification and release timing remain controlled by Microsoft. If submission fails, the CI summary reports the failed Store stage; the GitHub Release job remains independent so it can still publish the verified cross-platform assets.

## Verification

The workflow checks that Store submission settings are either all present or all absent and rejects enabled submission without MSIX identity configuration. Synthetic PowerShell cases cover disabled, enabled, partial, missing-secret, missing-product-ID and missing-MSIX configurations without live credentials. The Microsoft action and Store endpoint are external; live authentication, a successful Partner Center submission and certification require an already-published free app and are not locally verified.

## References

- [Publish app updates to Microsoft Store with GitHub Actions](https://learn.microsoft.com/en-us/windows/apps/publish/msstore-dev-cli/github-actions)
- [Manage Microsoft Entra applications in Partner Center](https://learn.microsoft.com/en-us/windows/apps/publish/partner-center/manage-azure-ad-applications-in-partner-center)
- [Microsoft Store submission API](https://learn.microsoft.com/en-us/windows/apps/publish/store-submission-api)
