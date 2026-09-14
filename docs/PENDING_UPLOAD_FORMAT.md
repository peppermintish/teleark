# Pending upload envelope v1

The independent pending-upload codec reads/writes **1**, advertised by remote
caption `TeleArk pending upload v1` and filename `<32-hex-package-id>.tarku`.
It announces an incomplete logical file. It is never a completed manifest or a
portable Telegram temporary-part bitmap. All fields and lengths are authenticated;
source paths, sessions and bare keys are absent. The source file name is encrypted.

## Encoding

Integers are big-endian. Exact length is required; no trailing data is permitted.

| Offset | Field | Encoding |
| --- | --- | --- |
| 0 | Magic | 8 bytes `TARKUPL\0` |
| 8 | Version | u16, exactly 1 |
| 10 | Flags | u16, bit 0 means whole-source hash known; all other bits rejected |
| 12 | Public header length | u32 |
| 16 | Encrypted metadata length, excluding tag | u32 |
| 20 | Telegram account ID | positive i64 |
| 28 | Telegram channel ID | positive i64 |
| 36 | Update salt | 32 OS-random bytes, fresh for every seal |
| 68 | Public header | Existing canonical manifest public-header CBOR |
| next | Encrypted metadata | Existing canonical manifest metadata CBOR, encrypted |
| final | Authentication tag | 16 bytes |

The public header declares the complete logical size, container count, persisted
plaintext target, aligned-frame geometry, package/vault IDs and wrapped File Key.
Its manifest-generation field is 1 inside a pending envelope and `flags` is 1 for
the aligned frame layout. It does not authorize completed-manifest generation 1.

The metadata contains a contiguous, ordered prefix of published containers. Each
entry has the existing authenticated lengths, offsets, plaintext/encoded BLAKE3,
frame count, part instance and scoped remote locator. Prefix length cannot exceed
the declared file or part count. With flags=0 the prefix must be empty and the
whole-source hash must be zero. Flags=1 supplies the actual whole-source hash;
an empty published prefix is then valid. These states never become executable
source admission merely because a zero digest was present.

Encryption uses AES-256-GCM with a zero 96-bit nonce and a unique per-update key:
HKDF-SHA256(File Key, salt=update salt,
info=`teleark/pending-upload/v1` || package ID), output 32 bytes. AAD is the entire
68-byte prefix plus public-header bytes. Domain separation and a fresh 256-bit salt
prevent pending updates from sharing completed-manifest/content keys or reusing a
key/nonce when ciphertext metadata changes. The account/channel binding also
prevents moving an announcement into another message-ID namespace for resumption.

Public headers are limited to 64 KiB, encrypted metadata to 64 MiB, with existing
canonical CBOR bounds (strings, nesting, arrays and at most 1,000,000 containers).
The desktop additionally limits a complete metadata transport object to 64 MiB.
Lengths are checked before allocation and arithmetic is checked before encoding.
Unrecognized versions, flags, suites, tampering and invalid prefixes fail closed.

## Resumption and persistence

Authentication requires the correct recovery-derived Vault Master Key. Runtime
checks the account, channel, vault, package and geometry; locators must be in the
same account/channel. It hashes the selected local file once for both hash levels,
rejects mismatches, imports the prefix atomically with queued-job promotion, and
stream-verifies remote containers before reusing them. Remaining containers use
fresh encryption identities. A known hash cannot be replaced by a conflicting
update. Discovery chooses authenticated progress and hides pending logical rows
after successful completed-manifest discovery; raw remote messages remain.

A local 80-byte `TARKRI01` locator remembers an announcement during source
preparation: magic8, little-endian positive message ID i64, BLAKE3-256 of the exact
pending-source admission bytes, then BLAKE3-256 of the preceding 48 bytes. It is
atomically replaced and synced in the private task metadata directory. The
checksum detects accidental corruption; remote AEAD authentication and local
account/generation checks remain mandatory. Unknown/corrupt locators are retained,
not silently replaced. They contain neither plaintext payload nor encrypted data
blocks. Other recovery records are documented in
[encrypted transfer recovery](VAULT_TRANSFER_RECOVERY.md).

The scope is handoff of the same file within one Telegram account/storage channel.
Temporary part retention is not a remote durability promise. Interrupted initial
publication can leave an orphan announcement if its reply/local locator was lost;
it remains explicitly incomplete and is discoverable remotely. No automatic
remote-message deletion or distributed exclusive-writer lease is provided.
