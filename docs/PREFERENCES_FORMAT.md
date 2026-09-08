# Desktop Preferences Format

Status: implemented local format, version 1

TeleArk stores desktop preferences in the Library SQLite `settings` table. The
frontend exchanges one typed `DesktopPreferences` value with the runtime and
never reads these rows directly. A save validates the entire value and writes
all keys in one storage transaction.

## Namespace and scalar encoding

Version 1 uses the `preferences.v1.` prefix. Boolean values are exactly `true`
or `false`; integers are canonical base-10 strings; appearance is `system`,
`light`, or `dark`. `managed_files_root` is a UTF-8 path, with the empty string
selecting TeleArk's platform application-data directory. The runtime derives
`Downloads`, `Cache`, and `Logs` children from this root and creates them together.

| Key suffix | Allowed value | Default |
| --- | --- | --- |
| `managed_files_root` | UTF-8 path or empty | empty |
| `download_directory` | retired UTF-8 path or empty | empty |
| `ask_download_destination` | retired boolean | `false` |
| `reveal_completed_downloads` | boolean | `false` |
| `upload_part_size_mib` | retained legacy value: `1024` or `1900` | `1900` |
| `upload_encrypt_content` | retained legacy boolean | `true` |
| `upload_hide_file_name` | retained legacy boolean | `true` |
| `upload_encrypt_metadata` | retained legacy boolean | `true` |
| `download_throughput_strategy` | `balanced` or `max_throughput` | `balanced` |
| `transfer_soft_limit_policy` | `respect`, `adaptive_override`, or `ignore` | `adaptive_override` |
| `lock_vault_when_hidden` | boolean | `true` |
| `index_batch_size` | `200`, `500`, or `1000` | `1000` |
| `notify_download_completed` | boolean | `true` |
| `notify_download_failed` | boolean | `true` |
| `sidebar_collapsed` | boolean | `true` |
| `channel_sidebar_width` | decimal integer, 180–720 logical pixels | `208` |
| `appearance` | `system`, `light`, or `dark` | `system` |

## Compatibility and failure behavior

Missing keys take their documented defaults, which lets older databases adopt
new version-1 settings. When `managed_files_root` is absent, a non-empty legacy
`download_directory` becomes the managed root; if its final component is
`Downloads` (case-insensitive), its parent becomes the root so the existing
download path is preserved. A subsequent save writes the new key and clears the
retired destination values. The retired
`ask_download_destination` value is ignored because downloads are now always
automatic. Unknown suffixes in the version-1 namespace are ignored for forward
compatibility. A malformed active known key makes the runtime report a
structured persistence error; the desktop falls back to defaults and shows
that settings could not be loaded or saved. A future incompatible encoding
must use a new namespace and migration rather than changing version 1 in place.
Supported upgrades perform that migration automatically under Runtime ownership,
preserving a recoverable prior value set and reporting expensive preparation or
conversion to the UI. Users should not need to recreate settings. See
[ADR 0017](adr/0017-versioned-automatic-migrations.md); the existing legacy-value
read/save conversion below is the current implementation baseline.

The four retained `upload_*` keys remain parseable so existing version-1
preferences do not break, but the connected private-channel writer does not use
them. Vault uploads currently require encryption of content, original name, and
manifest metadata and use the runtime's conservative 60 MiB plaintext-part
ceiling. Those protections are presented only in the TeleArk upload
flow; Settings no longer exposes inactive controls that would imply otherwise.

`transfer_soft_limit_policy` affects only official conservative guidance, never
Telegram protocol limits or structured FloodWait deadlines. `respect` refuses
to cross a known soft active-file limit, `adaptive_override` probes across it
and keeps the change only when measured goodput justifies it, and `ignore`
allows the normal probe order to cross it. Every conflict is a typed controller
decision and a session-log event. Missing older values adopt the recommended
`adaptive_override` default.

The managed root is not a permission grant. The runtime creates only its
`Downloads`, `Cache`, and `Logs` children, validates requested filenames, refuses path
traversal, and chooses a unique `name (n).extension` download destination
instead of replacing an existing file. Candidates also skip destinations retained in native transfer history, even after their local files are deleted. SQLite and the Telegram session stay in
the platform application-data directory so changing this preference cannot
move files that are open by live workers.

`download_throughput_strategy` is an additive v1 key, independent of the soft
active-file policy. Missing legacy values retain Balanced (P4–24); older readers
ignore the new key. Max Throughput starts native downloads at P4 (search range P1–64) and probes
with one-second settling for strong gains and five-second confirmation otherwise:
initial steps up to 16 shrink near a measured
throughput/error boundary, down to one part. The preference is captured once
when a native task starts/resumes/retries; it does not change an already running
owner or the encrypted Vault pipeline. Unknown values fail as structured
persistence errors. Tests cover literal legacy rows, invalid values, and a
Max Throughput save/reopen round trip. See ADRs [0010](adr/0010-native-download-throughput-strategy.md) and [0011](adr/0011-adaptive-native-probe-refinement.md).

`channel_sidebar_width` is an additive version-1 setting. Older databases use
208 without rewriting existing values; older readers ignore the new suffix.
The GUI adjusts the list immediately while dragging and saves the final width
through the background preferences owner on release. Window-size constraints
reserve at least 500 logical pixels for channel content at supported window
sizes and do not rewrite the stored preference. Invalid values fail validation
before any settings write. Navigation or window deactivation discards unfinished
drags; the last completed width is restored when the channel list reopens.

## Account-scoped private channel binding


`storage-channel.v1.account.<account-id>` retains its canonical positive decimal
chat ID encoding as a cache for compatibility. Desktop automatic management
never reads it to establish identity or authorize creation/repair. Successful
remote verification replaces a stale cache. No local creation-intent key exists.

Remote text is explicit UTF-8, not Rust/Serde layout. The v2 description is:

```text
teleark:storage:v2
identity:<positive canonical decimal i32 message ID>
<localized warning, at most 200 Unicode scalar values>
```

The identified pinned, non-forwarded message has this v1 record:

```text
teleark:channel-identity:v1
account:<current positive canonical decimal i64 account ID>
channel:<actual positive canonical decimal i64 channel ID>

<nonempty localized warning>
```

Writers use LF separators without a trailing LF. Readers require the exact
record prefix and account/channel values, and bound the whole message to 2,048
UTF-8 bytes. Description parsing uses lines and rejects noncanonical, zero,
negative, overflowing or unknown-version pointers. The warning body is display
text, not identity authority. Fixtures `storage-identity-v1.txt` and
`storage-description-v2.txt` in `teleark-telegram/tests/fixtures` freeze these
encodings. Existing `teleark:storage:v1` first-line descriptions remain a legacy
upgrade input only when remote title/ownership/privacy checks also pass; they
are not accepted for Vault writes until upgraded and reverified.

Discovery is bounded to a complete snapshot below 10,000 dialogs and 64 owned
or reserved-name channel inspections, including owned public channels. It checks
full creator/private broadcast metadata, no username/aliases, one participant
and administrator, no bots, linked discussion or message TTL. The description
pointer must resolve to a pinned original message whose account/channel IDs
match Telegram. Multiple candidates and damaged recognizable channels fail
closed. Setup is bounded to 60 seconds; Vault validation to 20 seconds. Legacy
initialization searches up to 64 identity-message results before sending/pinning
a record, writes the pointer and verifies again. No members, public username or
invitation are created. See [ADR 0015](adr/0015-remote-authoritative-storage-identity.md)
for atomic-creation and app-origin proof limitations.

`native-download-account-migration.v1` is a separate one-time migration marker.
Its writer emits `resolved`; presence prevents later reassignment of legacy
unknown download accounts. The assignment and marker commit atomically before
a configured connection is returned. See [Data model](DATA_MODEL.md) for schema
9 compatibility and unknown-provenance behavior.

`lock_vault_when_hidden` is a deprecated version-1 boolean setting. Readers and
writers preserve existing values for compatibility, but the current desktop
session policy no longer uses it to lock on inactivity or navigation. New
preferences default it to false. Explicit locking, account exit and process
exit end session access; already admitted background work keeps its own bounded
key lease. Existing bytes are not reinterpreted and no key/manifest/database
migration is needed. See [ADR 0026](adr/0026-session-unlock-and-task-key-leases.md).

## Network proxy policy: version 1

The SQLite `settings` key `network.proxy` stores a JSON object, independently versioned from preferences and app releases. Schema 14 creates `{"version":1,"mode":"direct"}` for existing/new installations. Current read/write codec is 1. No implicit direct default is permitted at runtime.

| Mode | Exact fields |
| --- | --- |
| `direct` | `version` (integer 1), `mode` |
| `socks5` / `http_connect` | `version`, `mode`, `address`, `username`, `password` |

`address` is a numeric socket address such as `127.0.0.1:1080` or `[::1]:8080`, with nonzero port and no unspecified/multicast IP. Hostnames are unsupported. Authentication fields are strings, each at most 255 UTF-8 bytes; an empty username requires an empty password. HTTP usernames cannot contain `:`. Unknown versions/modes/fields, invalid values and objects over 8,192 bytes fail closed. Writers first validate the existing record and preserve unsupported or corrupt bytes. The setting update is transactional; schema upgrades preserve all other data/keys and are restartable.

Synthetic compatibility fixtures live in `crates/teleark-runtime/src/fixtures/proxy-v1-*.json`. JSON field order is insignificant. Proxy authentication is stored in the protected local SQLite database, which is not encrypted; masked editor fields and sanitized diagnostics never imply encrypted-at-rest credentials. Password buffers in proxy configurations are zeroized on drop. [ADR 0024](adr/0024-fail-closed-proxy-routing.md) specifies route switching and failure behavior.
