# TeleArk Manifest Format

Status: **provisional, integrated v1 codec candidate**. `teleark-crypto` can
deterministically seal/open and strictly validate the candidate manifest, and
`teleark-runtime` now publishes it after verified parts, discovers it by a
common remote caption, recovers the File Key/layout/locators, and completes an
exact download with a fresh SQLite database in deterministic integration tests.
Exact bytes may change before v1 stabilization; after release, incompatible
changes require a new major version and old fixtures remain readable.

## Role

The manifest is the recovery backbone for a completed Vault package. SQLite is a cache/index/checkpoint store; losing it must not make a completed remote package unintelligible. With Telegram account/channel access and a valid password or Recovery Key, TeleArk should be able to scan manifest objects, validate/decrypt them, discover parts, rebuild logical files, and restore the local library.

Only completed packages have this disaster-recovery guarantee. Parts uploaded before a verified authoritative manifest are incomplete/orphan candidates and may require surviving local checkpoint/key state for reconciliation.

The current codec authenticates the envelope/public header, decrypts only after
File Key resolution, validates bounded canonical metadata and exact
part/container bindings, derives opaque remote names, and redacts sensitive
fields from `Debug`. The retained desktop Vault owner now uses those mechanics
for private-channel upload, authenticated managed-file discovery, and verified
restore. This remains an alpha workflow rather than a release recovery
guarantee because the format is provisional and credentialed crash/system and
independent security tests remain.

## Design properties

- explicit magic and version independent of Rust struct/Serde layout;
- strict deterministic encoding and parser bounds;
- a minimal public header sufficient to identify and safely decrypt;
- original name/path, hashes, remote locators, and rich metadata encrypted;
- authenticated binding between public header and encrypted metadata;
- one authoritative immutable manifest generation at a time;
- account-tier-independent application parts;
- safe rejection of unsupported formats and algorithms.

## Remote names

When hidden filenames are enabled, upload opaque ASCII names derived only from package identity, kind, version, and index. The provisional shape is:

```text
<lowercase-hex-package-id>.v1.manifest.tam
<lowercase-hex-package-id>.v1.000000.part.tav
<lowercase-hex-package-id>.v1.000001.part.tav
```

The provisional discovery captions paired with those names are:

```text
teleark-manifest-v1
teleark-object-v1-<lowercase-hex-package-id>-<eight-digit-lowercase-hex-part-index>
```

The raw TeleArk storage UI may use the exact name/caption pair to explain that
an object is a candidate manifest or part. This is classification only: it must
not expose encrypted metadata as trusted or claim a package is recoverable
until the bounded codec authenticates the manifest and validates its locators.

No original filename/path appears in the Telegram remote name or caption. Remote naming intentionally leaks package linkage, kind, format generation, and part ordering; it does not promise traffic-analysis resistance. A final naming alphabet/length and Telegram behavior must be verified before v1 fixtures are frozen.

## Envelope

The candidate envelope is:

```text
magic                       8 bytes   ASCII "TARKMAN\0"
format_major                u16       1
format_minor                u16       0
public_header_length        u32
encrypted_metadata_length   u64       ciphertext bytes, excluding 16-byte tag
public_header               deterministic CBOR bytes
encrypted_metadata          encrypted_metadata_length bytes
authentication_tag          16 bytes
```

CBOR follows the required RFC 8949 deterministic subset: definite lengths, shortest integer forms, canonical numeric map-key order, no duplicate keys, and no unsupported tags/floats. The candidate uses a small explicit bounded encoder/decoder rather than binding persistence to Serde or an in-memory Rust layout. Tests reject non-shortest integers, indefinite values, reordered/duplicate keys, unsupported types, trailing bytes, and excessive claims.

The exact envelope prefix plus exact canonical `public_header` bytes are AAD for metadata encryption. Authentication therefore covers magic/version/lengths and every public field.

## Public header schema

The deterministic CBOR public header uses unsigned integer keys; symbolic names below are documentation only. Required v1 fields are:

| Key | Field | Type / rule |
| ---: | --- | --- |
| 1 | `package_id` | 16-byte string |
| 2 | `vault_id` | 16-byte string |
| 3 | `manifest_generation` | unsigned 32-bit; immutable, monotonic per package |
| 4 | `created_at_unix_ms` | unsigned 64-bit UTC instant |
| 5 | `logical_file_size` | unsigned 64-bit plaintext bytes |
| 6 | `part_count` | unsigned 32-bit, bounded and consistent with metadata |
| 7 | `application_part_target` | unsigned 64-bit bytes; descriptive, not assumed for final part |
| 8 | `frame_plaintext_max` | unsigned 32-bit bytes |
| 9 | `nonce_strategy_id` | unsigned 16-bit; candidate strategy `1` |
| 10 | `crypto_suite_id` | unsigned 16-bit; candidate suite `1` |
| 11 | `file_key_wrap` | nested required map described below |
| 12 | `master_key_generation` | unsigned 32-bit |
| 13 | `flags` | unsigned 32-bit; all v1 bits currently zero/required understood |

`file_key_wrap` contains wrap algorithm ID, wrapped File Key ciphertext (32 bytes for suite 1), GCM tag (16 bytes), and any explicitly required generation/derivation metadata. The candidate derivation and zero nonce are defined in `CRYPTO_FORMAT.md`; nonce/key rules are not inferred from field absence.

The public header intentionally exposes approximate logical size, part/frame counts/parameters, package/vault linkage, and creation time. Original name/path, hashes, media metadata, and locators remain encrypted.

## Encrypted metadata schema

After file-key unwrap, derive the per-generation manifest key as specified in `CRYPTO_FORMAT.md` and decrypt the metadata. The plaintext is another deterministic CBOR map with required fields:

| Key | Field | Requirements |
| ---: | --- | --- |
| 1 | `logical_name` | UTF-8 original filename, bounded, no implicit normalization |
| 2 | `relative_path` | optional UTF-8 logical path; not trusted as a local filesystem path |
| 3 | `mime_type` | optional bounded UTF-8 |
| 4 | `media_kind` | stable locale-neutral value |
| 5 | `whole_plaintext_blake3` | exactly 32 bytes |
| 6 | `parts` | array of exactly `part_count` descriptors |
| 7 | `logical_timestamps` | optional structured timestamps with explicit semantics |
| 8 | `source_metadata` | optional bounded/versioned map; original user/source content only |
| 9 | `format_extensions` | optional extension map with defined criticality rules |

Each part descriptor contains:

```text
part_index                 u32, contiguous from zero
part_instance_id           16 bytes; agrees with authenticated part header
plaintext_offset           u64
plaintext_length           u64
encoded_length             u64
frame_count                u32
plaintext_blake3           32 bytes
encoded_ciphertext_blake3  32 bytes
remote_locator:
  account_id               stable TeleArk/Telegram account identity
  chat_id                  stable Telegram chat identity
  message_id               Telegram message identity
  remote_name              expected opaque name
  locator_version          adapter-owned codec version
  locator_extension        optional bounded adapter data
```

Volatile Telegram file references/access hashes are hints, not the sole durable identity. The adapter must be able to refresh/resolve locators from account/chat/message identity.

Descriptors must cover `[0, logical_file_size)` exactly, with no gaps or overlap. Part count, container headers, indices, lengths, frame parameters, package ID, and remote names must agree. The final part may be shorter than the target; all non-final layout rules are validated explicitly rather than inferred solely from `application_part_target`.

## Authentication and key resolution

1. Parse and bound-check the fixed envelope/public header without trusting it.
2. Select a supported major version, suite, and master-key generation.
3. Derive the package wrap key and authenticate/unwrap the File Key.
4. Derive the unique manifest-generation key.
5. authenticate/decrypt metadata with exact envelope/public-header bytes as AAD.
6. Decode deterministic metadata and validate all cross-field/package invariants.

Public-header tampering changes AAD or key derivation and fails authentication. Wrong password/recovery/master/file key is reported as a structured authentication/key error without accepting any metadata. Parser errors do not expose partially decoded names as trusted library items.

## Compatibility rules

- Unknown `format_major` is safely rejected as `UnsupportedManifestVersion` while preserving bytes for a future reader.
- A reader may accept a higher minor version only if every added field/flag is explicitly optional and noncritical under v1 rules.
- Unknown suite/required flag/critical extension is rejected, never guessed.
- Unknown noncritical metadata fields may be ignored after successful authentication; rewrite tools should preserve them when the codec policy requires roundtripping.
- Writers emit one canonical representation for a version.
- Released readers retain fixtures for all supported versions; internal struct changes cannot alter encoded output accidentally.

Provisional parser limits for v1 are: public header at most 64 KiB, encrypted metadata at most 64 MiB, part count at most 1,000,000, bounded strings/arrays/nesting, and checked total sizes. Final values require recovery-scale tests and may become stricter. Limits are applied before allocation and must not conflict with valid stated maximum file/part geometry.

## Upload publication and authority

Content parts upload and verify first. The manifest is generated from verified remote locators, encrypted, uploaded, and verified last. Only then may the package transition to `Completed` and its manifest become authoritative.

If the process crashes:

- before remote part success: resume from durable local state;
- after remote part success but before checkpoint commit: discover the deterministic package/part identity and repair local state;
- after all parts but before manifest publication: treat the package as incomplete; do not invent a completed manifest without surviving key/source/checkpoint evidence;
- after manifest upload but before local commit: rediscover and validate the manifest, then repair the database.

Manifest replacement uses a new generation and explicit authority/update rules. Never silently overwrite bytes under an existing immutable generation/key/nonce identity.

## Database-loss recovery

```text
authenticate Telegram account
 -> select/confirm storage channel
 -> scan objects matching supported manifest identification
 -> strictly parse bounded public headers
 -> select vault and unlock master key through password/recovery path
 -> authenticate/decrypt manifests
 -> validate package layouts and remote locators
 -> verify/discover referenced parts
 -> rebuild SQLite logical files, package, parts, and search records
 -> report incomplete/corrupt/unsupported packages separately
```

Do not trust remote filenames alone. Discovery name/caption pairs are only
candidates; the envelope and manifest must still authenticate. Duplicate
package IDs/generations, conflicting manifests, missing parts, cross-account
locators, and unexpected objects require deterministic conflict handling and
user-visible structured status. Recovery never deletes remote objects automatically.

## Candidate fixtures and remaining recovery tests

A fixed Unicode multipart fixture now lives under `crates/teleark-crypto/tests/vectors/manifest_v1/`; deterministic tests also cover empty manifests. The suite covers encode/decode, wrong keys, envelope/header/metadata tamper, duplicate/noncanonical keys, unknown versions/suites/flags, length/offset overflow, excessive allocation claims, invalid UTF-8, unsafe relative paths, duplicate/reordered/missing/overlapping parts, locator/name mismatch, part-container binding, duplicate manifest-generation identity, and hostile mutation/truncation corpora.

Before stabilization, add released-version compatibility fixtures, longer
recorded fuzz campaigns, conflict/generation recovery policy tests, credentialed
Telegram system recovery, and independent security review. The deterministic
acceptance test now creates a fresh SQLite database, recovers through the
authenticated remote Manifest without a caller-supplied File Key/layout,
downloads/decrypts, and proves the recovered BLAKE3/plaintext exactly matches
the original.
