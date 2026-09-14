# ADR 0035: Vault source metadata and content validation

Status: Accepted.

## Problem

Vault preparation compared every field of `SourceIdentity` for equality. On Unix,
`revision` records ctime, which changes for filesystem metadata updates as well as
content writes. An unchanged file could therefore become permanently blocked after
admission hashing, before any encryption reservation or remote upload.

## Decision

Use one Vault-specific comparison for filesystem identity, size and content
modification time, with the existing separate canonical-path checks. Preserve ctime
in the saved identity, but do not treat its change alone as proof of changed content.
This rule applies at queue validation, after initial hashing, before publication and
when revalidating a saved upload context. Generic transfer identity equality is
unchanged.

Metadata screening never replaces content authentication. Initial admission hashes
the complete file and every part before reserving encryption identities. Encryption
compares each part's bytes with its immutable digest before nonce use. Publication
checks the complete streamed digest and verifies remote ciphertext, authenticated
plaintext and layout. Resume requires the saved complete source digest, so changing
bytes while preserving size and mtime cannot authorize nonce reuse.

## Compatibility and verification

Context and pending-source codecs remain read/write v1. Field encodings and meaning,
including ctime, do not change; readers retain existing bytes and immutable recovery
records. No migration or conversion is necessary. Previously blocked failures keep
their intent and require a fresh selection; no failed file is silently resent.

Deterministic tests change native file permissions during queued preparation,
between hashing reads, during blocked transport and before restart. Successful
uploads download and authenticate the original bytes. A resumed file with different
same-size bytes and restored mtime is rejected while preserving the original context
and part reservations. Existing cancellation, stale-generation, tamper and frozen-v1
codec tests continue to apply. This does not claim protection against an adversarial
filesystem that replaces files and restores all metadata during a first admission.
