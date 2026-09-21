# ADR 0046: Tag-only push trigger

Status: Accepted. Supersedes ADR 0044's branch-push trigger; retains its single workflow and the release package contract from ADR 0045.

## Decision

The one CI/CD workflow triggers automatically on pull requests and `v*` tag pushes, and supports manual dispatch. It does not trigger on branch pushes. A combined branch and version-tag push therefore starts one release run rather than separate branch and tag runs. The tag run still performs LF, legal and source quality checks before building and publishing packages. Pull requests and ordinary manual runs perform the Windows and macOS test matrix without building installers.

## Tradeoff

Direct pushes to `main` no longer receive automatic CI checks. Contributors must use a pull request or manual dispatch to check an untagged commit. A version tag checks its exact commit before publication.
