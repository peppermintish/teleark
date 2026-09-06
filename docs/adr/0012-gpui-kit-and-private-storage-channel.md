# ADR 0012: GPUI Kit desktop and dedicated private storage

- Status: Accepted
- Date: 2026-09-07
- Supersedes: the GUI version pins in ADR 0007; all its provenance rules remain.

## Decision

Use the published `gpui-kit = 0.6.0` facade, locked to the matching
`gpui-pre-* = 0.3.3` family. Initialize the platform through `application()`
and the enabled layers through `init()`. GPUI Base supplies behavior and
accessibility; Component supplies styled controls, overlays and DataTable.
Do not introduce a JavaScript runtime, webview, or unrelated editor features.
Core and all runtime adapters remain independent of GPUI.

The published Kit, Base, Component, Assets and gpui-pre packages declare
Apache-2.0. In particular, the registry's versioned `gpui-pre-zlog`,
`gpui-pre-ztracing` and `gpui-pre-ztracing-macro` 0.3.3 are Apache-2.0;
the old unprefixed GPL packages remain banned. This is a reviewed registry
snapshot, not authorization to adopt arbitrary Zed application code.
The snapshot is young and has no stable API promise; retain the lockfile,
upstream attribution and dependency gate. No prohibited source was consulted.

Two new transitive license identifiers were reviewed from the shipped license
texts: `libbz2-rs-sys 0.2.5` (bzip2-1.0.6, permissive redistribution with
attribution/origin conditions) and `webpki-roots 1.0.9`
(CDLA-Permissive-2.0, certificate data redistribution with license text).
Exact-package exceptions and complete texts are retained in deny.toml and
THIRD_PARTY_NOTICES.md. Advisory, source and GPL bans remain enabled.

Transfers and TeleArk storage are stable primary destinations. Channels scroll
independently; refreshing them never navigates away from the current route.
Progressive disclosure keeps index, protocol and controller details available
without occupying the primary file and transfer workspace. Authentication has
a dedicated welcome surface for fresh login and a returning-account surface.
Vault prerequisites are actionable and preserve the interrupted navigation.

New encrypted packages belong in a dedicated private broadcast channel owned
by the authorized account. Saved Messages is never a new upload destination.
Channel identity is account scoped, checked by the adapter, and discoverable
by a versioned description marker rather than a translated/display title.
Raw documents and authenticated logical files are two projections of this
channel. No crypto or manifest bytes change as part of this destination change.
Existing Saved Messages packages remain recoverable through an explicit
legacy recovery entry; they are never moved or deleted implicitly.

## Durable account and discovery state

SQLite migration 9 adds a nullable positive `account_id` to native task history.
New tasks require an account; a batch cannot mix accounts. Before returning the
first configured Telegram connection, the runtime resolves old NULL rows once:
assign the restored session's actual account, or retain NULL when unauthorized.
The same storage transaction writes `native-download-account-migration.v1 =
resolved`. Later logins cannot claim those unknown rows. Native scheduling,
resume, retry and actual network execution check the expected account. Switching
pauses native work and waits for retained workers before signing out; an active
Vault transfer must finish first. Existing completed local files are retained.

Bindings use `storage-channel.v1.account.<canonical-positive-account-id>` with
a canonical positive chat-ID value in SQLite settings. Remote descriptions start
with the exact line `teleark:storage:v1`; ownership/private broadcast metadata
must independently validate. Titles are user content, not identity. Discovery
is bounded to 10,000 dialogs / 64 candidates and fails on truncation, ambiguous
creation responses or incomplete checks. Create never blindly retries.
Manifest scans remain bounded to 1,000 candidates and now support cancellation
through the GUI, runtime, and serialized Telegram owner; cancellation is not
reported as a rejected manifest. Existing crypto, manifest, native bitmap and
session-log encodings are unchanged.

## Sources

- https://docs.rs/gpui-kit/0.6.0/gpui_kit/
- https://docs.rs/gpui-base/0.6.0/gpui_base/
- https://docs.rs/gpui-pre/0.3.3/gpui/
- https://core.telegram.org/method/channels.createChannel
- https://core.telegram.org/method/channels.getFullChannel
- https://docs.rs/grammers-client/0.10.0/grammers_client/
