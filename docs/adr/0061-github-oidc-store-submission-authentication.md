# ADR 0061: GitHub OIDC for Store submission authentication

Status: Accepted — 2026-09-30. This supersedes the authentication mechanism in [ADR 0060](0060-windows-msix-only-store-release.md).

## Context

Microsoft Store Developer CLI v0.4.2 includes client-assertion authentication. Its `reconfigure --clientAssertion` option reads the assertion from `MSSTORE_CLIENT_ASSERTION`. The prior Store workflow passed `AZURE_AD_APPLICATION_SECRET` in the CLI process arguments, which violates the repository credential-handling rule.

The Microsoft Learn command reference documents a client secret or certificate for `reconfigure`; the official CLI v0.4.2 source and tests add client-assertion support. The Store CLI does not provide an environment-variable input for the existing client secret. GitHub Actions can issue an OIDC token that Microsoft Entra ID exchanges through a federated identity credential.

## Decision

Pin the Store CLI to v0.4.2 and use its client-assertion mode. The release job requests a fresh GitHub Actions OIDC token with audience `api://AzureADTokenExchange` before each CLI command. It sets that token as `MSSTORE_CLIENT_ASSERTION` in the current process environment, then clears the assertion and GitHub OIDC request environment values after the command.

The job uses the GitHub Environment `store-submission` and grants `id-token: write` only to that job. Its Entra federated credential uses issuer `https://token.actions.githubusercontent.com`, audience `api://AzureADTokenExchange`, and immutable subject `repo:peppermintish@127260537/teleark@1364331653:environment:store-submission`. Restrict the GitHub environment to version-tag release refs as documented in [Development](../DEVELOPMENT.md#microsoft-store-github-oidc-authentication).

The release workflow continues to require the existing tenant, client ID, seller ID, and Store Product ID settings. It no longer requires or passes `AZURE_AD_APPLICATION_SECRET`.

## Consequences

The Entra app registration must contain the matching federated identity credential before Store submission can authenticate. Store submission remains a required job before GitHub Release publication. A missing or mismatched trust entry fails the Store job and prevents the GitHub Release.

The OIDC assertion is not included in CLI arguments or repository files. Microsoft Store CLI persists the client-assertion mode flag, not the assertion value.

## Verification

Synthetic tests cover token audience construction, bearer-header placement, assertion delivery through the process environment, command-argument redaction, cleanup, and failure-message redaction. The tests do not call GitHub's OIDC endpoint, Microsoft Entra ID, Partner Center, or the Microsoft Store.

## References

- [Microsoft Store Developer CLI commands](https://learn.microsoft.com/en-us/windows/apps/publish/msstore-dev-cli/commands)
- [Microsoft Entra: configure an app to trust an external identity provider](https://learn.microsoft.com/en-us/entra/workload-id/workload-identity-federation-create-trust)
- [GitHub Actions OpenID Connect reference](https://docs.github.com/en/actions/reference/security/oidc)
