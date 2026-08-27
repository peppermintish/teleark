# TeleArk Crypto and Manifest Audit Package

Status: ready for external scoping; no independent audit has been completed.

## Required independence

The final reviewer must be organizationally independent from the implementation
work, disclose conflicts, identify the exact commit reviewed, and sign a report
that separates confirmed findings, residual risks, and out-of-scope areas.
Internal tests and this package are preparation, not an independent audit.

## Review scope

- `crates/teleark-crypto`, including part, manifest, key-wrap, KDF, nonce, and
  AEAD-usage-registry logic;
- `docs/CRYPTO_FORMAT.md`, `docs/MANIFEST_FORMAT.md`, `docs/SECURITY.md`, and
  their candidate vectors under `crates/teleark-crypto/tests/vectors`;
- parser bounds, canonical encodings, downgrade/version behavior, key
  separation, nonce uniqueness, password parameters, redaction, tamper
  handling, and recovery authority;
- transfer integration once production streaming and recovery adapters exist.

## Evidence to reproduce

```text
cargo test -p teleark-crypto
cargo clippy -p teleark-crypto --all-targets -- -D warnings
cargo fuzz run part_decode
cargo fuzz run manifest_decode
```

Scheduled CI runs both fuzz targets for ten minutes every day with bounded
memory and preserves crash artifacts. A release candidate must additionally
record a materially longer campaign, toolchain, corpus hash, executions,
coverage, peak memory, and every minimized/replayed finding.

The fuzz-only dependencies were reviewed at introduction: `cargo-fuzz 0.13.2`
declares MIT OR Apache-2.0, and `libfuzzer-sys 0.4.13` declares the conjunction
of MIT/Apache-2.0 and the permissive NCSA license. They are CI/development tools
and are not linked into TeleArk release binaries.

## Format-stability gate

`FORMAT_MAJOR = 1` remains a candidate identifier, not a compatibility promise.
The provisional marker may be removed only after all of the following exist:

1. signed independent review with all critical/high findings resolved;
2. long-running fuzz evidence and retained regression corpora;
3. cross-implementation or independently generated vectors;
4. integrated upload, database-loss discovery, authenticated download, and
   byte-for-byte recovery tests;
5. a release ADR freezing canonical bytes and backward-read obligations.

The reviewer should return findings with severity, affected bytes/API,
exploitability, reproduction, remediation, and retest result. The project must
retain the report hash and reviewed commit in the release record.
