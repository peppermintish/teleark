# ADR 0005: Use One Free-Account-Compatible Application Part Strategy

- Status: Accepted target; Telegram behavior must be revalidated before production
- Date: 2026-08-24

## Context

TeleArk must not depend on Telegram Premium limits or change package layout when account tier changes. Large logical files need application-level multipart storage below the ordinary per-file limit. Premium-specific 4 GB behavior would harm portability and recovery assumptions.

## Decision

Use a single compatibility-oriented application part strategy with a default target of **1900 MiB**, subject to reduction if current official Telegram/`grammers` behavior proves a lower safe limit. The format and recovery path are account-tier-independent. The GUI exposes automatic multipart/compatibility mode and does not offer a Premium mode.

Application parts contain smaller authenticated crypto frames and stream through `grammers`; they are not MTProto protocol parts. The final part may be shorter. Exact size is recorded in each manifest rather than inferred from account state.

## Consequences

- Packages remain readable after Premium status changes and across ordinary supported accounts.
- Very large files create more remote objects than a Premium-maximized strategy.
- `1900 MiB` is a binary exact format/configuration quantity and must not be mislabeled as `1900 MB`.
- Current Telegram API limits and `grammers` support must be verified from clean-room authoritative sources before real upload implementation.
- Tests use much smaller injected sizes while preserving layout invariants.
- Changing the default does not reinterpret existing manifests; each records actual part layout.
