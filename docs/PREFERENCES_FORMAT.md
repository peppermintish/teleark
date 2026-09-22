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
| `upload_speed_limit_bytes_per_second` | unsigned 64-bit integer; `0` means unlimited | `0` |
| `download_speed_limit_bytes_per_second` | unsigned 64-bit integer; `0` means unlimited | `0` |
| `download_throughput_strategy` | retained legacy: `balanced` or `max_throughput`; inactive | `balanced` |
| `transfer_soft_limit_policy` | retained legacy: `respect`, `adaptive_override`, or `ignore`; inactive | `adaptive_override` |
| `manual_transfer_v1` | nine comma-separated integers, order and bounds below | `3,10,2,2,4,3,8,2,4` |
| `lock_vault_when_hidden` | retired and ignored | none |
| `index_batch_size` | retained legacy value: `200`, `500`, or `1000`; inactive | `1000` |
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

The transfer-speed extension reads and writes these two additive version-1 integer
keys. Missing keys (including upgrades that skip releases) automatically select
unlimited, preserving prior transfer behavior; no existing value is reinterpreted
and no schema, encrypted-file, recovery-bundle or checkpoint version changes.
Version-1 readers predating this extension ignore the optional caps. A save writes
the complete set transactionally, then updates both live budgets in the same
storage-owner order. Preparation or persistence failure leaves live limits and
stored values unchanged. Malformed speed values remain intact: library access is
available for repair, preferences report a structured error, and configured
network startup fails closed until the settings are corrected. Limits are local
application preferences and are shared across all current transfer tasks, rather
than copied into per-task recovery records.

The four retained `upload_*` keys remain parseable so existing version-1
preferences do not break, but the connected private-channel writer does not use
them. Vault uploads currently require encryption of content, original name, and
manifest metadata and use the runtime's conservative 60 MiB plaintext-part
ceiling. Those protections are presented only in the TeleArk upload
flow; Settings no longer exposes inactive controls that would imply otherwise.

The two legacy profile/advisory keys are retained and validated for compatibility,
but no production transfer reads them to choose parameters. Settings exposes
manual controls and the existing shared byte-per-second caps.

The managed root is not a permission grant. The runtime creates only its
`Downloads`, `Cache`, and `Logs` children, validates requested filenames, refuses path
traversal, and chooses a unique `name (n).extension` download destination
instead of replacing an existing file. Candidates also skip destinations retained in native transfer history, even after their local files are deleted. SQLite and the Telegram session stay in
the platform application-data directory so changing this preference cannot
move files that are open by live workers.

`manual_transfer_v1` independently versions the manual parameter tuple. In order:

| Parameter | Default | Inclusive bounds |
| --- | ---: | ---: |
| Upload files | 3 | 1–8 |
| Concurrent 512 KiB upload parts per file | 10 | 2–64 |
| Shared upload MTProto connections | 2 | 1–8 |
| Ready ciphertext queue blocks per file | 2 | 1–16 |
| Upload part attempts, including the first | 4 | 1–10 |
| Download files, shared by native/Vault owners | 3 | 1–8 |
| Concurrent download logical parts per file | 8 | 1–64 |
| Shared download MTProto connections | 2 | 1–8 |
| Download part attempts, including the first | 4 | 1–10 |

Missing tuples automatically adopt these defaults, including skipped upgrades;
existing keys and encrypted bytes are not reinterpreted. All nine values validate
and save transactionally. Malformed tuples remain intact and fail loading. A new
incompatible tuple needs a new versioned key and migration. Running part pipelines
capture their settings at admission; a changed download file limit wakes pending
owners and limits new admissions without cancelling existing work. Upload window
admission captures the selected file limit. Server deadlines and fixed protocol
framing are constraints, not automatically tuned user parameters.

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

`lock_vault_when_hidden` is retired and ignored. Application PIN configuration is independent of desktop preference saves.

## Network proxy policy: version 1

The SQLite `settings` key `network.proxy` stores a JSON object, independently versioned from preferences and app releases. Schema 14 creates `{"version":1,"mode":"direct"}` for existing/new installations. Readers retain codec 1; authenticated proxy policies automatically migrate to credential-reference codec 2. Direct routing remains codec 1. No implicit direct default is permitted at runtime.

| Mode | Exact fields |
| --- | --- |
| `direct` | `version` (integer 1), `mode` |
| `socks5` / `http_connect` | `version`, `mode`, `address`, `username`, `password` |

`address` is a numeric socket address such as `127.0.0.1:1080` or `[::1]:8080`, with nonzero port and no unspecified/multicast IP. Hostnames are unsupported. Authentication fields are strings, each at most 255 UTF-8 bytes; an empty username requires an empty password. HTTP usernames cannot contain `:`. Unknown versions/modes/fields, invalid values and objects over 8,192 bytes fail closed. Writers first validate the existing record and preserve unsupported or corrupt bytes. The setting update is transactional; schema upgrades preserve all other data/keys and are restartable.

Synthetic compatibility fixtures live in `crates/teleark-runtime/src/fixtures/proxy-v1-*.json`. JSON field order is insignificant. Proxy authentication is stored in the selected Keychain/SQLite credential backend; the local SQLite option is not encrypted; masked editor fields and sanitized diagnostics never imply encrypted-at-rest credentials. Password buffers in proxy configurations are zeroized on drop. [ADR 0024](adr/0024-fail-closed-proxy-routing.md) specifies route switching and failure behavior.

## Channel directory restart cache

The account-scoped channel directory has its own version-1 cache codec in the settings table; it is not a preference field or a Telegram cursor. [ADR 0031](adr/0031-automatic-account-synchronization.md#bounds-and-compatibility) specifies header/page keys, bounds, atomic replacement, automatic legacy fallback and unsupported-version preservation. SQLite and existing preference formats retain their meanings.


## Application PIN envelope v1

The optional `application.pin` row is separate from `preferences.v1.*`. Absence
means no application lock. The JSON object has integer `version: 1` and byte-array
`verifier`, an existing v1 authenticated password-wrap encoding of a disposable
random key. It contains no PIN or file encryption key. See
[ADR 0037](adr/0037-application-pin-and-transfer-drain.md) and the synthetic
[v1 fixture](../crates/teleark-runtime/src/fixtures/app-pin-v1.json).
Current readers/writers support only envelope v1. Malformed/newer records are
preserved and keep the access gate closed. Settings saves do not rewrite the PIN;
PIN changes use a separate serialized compare-and-replace operation.

Indexing is automatic. The legacy `index_batch_size` value remains readable for compatibility, but does not control indexing. Runtime scanning uses a bounded automatic batch; Settings exposes no indexing controls. Download notifications are ordinary General settings.

Credential-reference proxy codec 2 is documented in [Data model](DATA_MODEL.md#credential-storage-schema-23). The Keychain preference is owned separately from ordinary desktop preferences so unrelated settings saves cannot change credential storage.
