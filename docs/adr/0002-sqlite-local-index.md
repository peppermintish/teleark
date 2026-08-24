# ADR 0002: Use SQLite and FTS5 as the Local Index

- Status: Accepted
- Date: 2026-08-24

## Context

A channel may contain more than one million messages. Searching Telegram history remotely for each query is too slow, network-dependent, difficult to facet, and unsuitable for offline library behavior. TeleArk also needs migrations, index-range evidence, transfer checkpoints, collections, and deterministic queries. At the same time, local database loss must not make completed Vault packages unrecoverable.

## Decision

Use SQLite as the local structured store and SQLite FTS5 for filename/caption/tag full-text search where appropriate. The storage adapter owns SQL, migrations, transactions, WAL/foreign-key/busy-timeout configuration, batch writes, query plans, and keyset pagination.

SQLite is an index, cache, settings/checkpoint store, and search infrastructure. It is not the sole recovery authority for completed Vault data; versioned remote manifests and parts retain that role.

## Consequences

- Search and facets can operate locally at million-record scale without live Telegram scans.
- Deployment remains embedded and transactional, but schema migrations and concurrency behavior need rigorous tests.
- FTS tokenization for Chinese, Japanese, emoji, combining text, and case must be measured.
- Historical coverage requires `index_ranges`, not one `last_message_id`.
- Deep `OFFSET` is rejected in favor of stable keyset/cursor pagination.
- Empty-to-latest and released-version migration tests must prove data preservation.
- Deleting/rebuilding SQLite may lose local-only state but must not prevent recovery of completed Vault packages.
