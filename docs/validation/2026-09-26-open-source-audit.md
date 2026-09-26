# Open-source preparation audit — 2026-09-26

Baseline: `a659f88` (`v0.5.8`). This review covers tracked documentation, scripts,
workflow configuration and all locally reachable Git history. It does not change
repository visibility, publish packages, push refs or rewrite history.

## Scope and findings

At the baseline, 212 commits were reachable across local refs, with 183 on `main`.
There were 127 annotated tags: 32 release tags and 95 task/checkpoint tags. Main is
linear; no merge cleanup is required. Twenty tags, including `v0.4.4`, point outside
main's ancestry. Commit metadata, ref relationships and historical file inventory
were checked across that scope; selected substantive diffs were inspected. Every
historical revision was not rebuilt or independently security-audited.

A heuristic scan of 2,867 reachable blobs found no private-key blocks or common
API-token patterns requiring credential rotation. Telegram hash candidates were
repeated-character synthetic fixtures. No private environment or signing-key paths
were tracked. This is not proof that history is secret-free: custom credentials,
obfuscated values and historical external artifacts need separate consideration.
The checked-in macOS certificate is public material, not a private signing key.

Current dual-license files, third-party notices and explicit dependency-policy
exceptions are present. The current dependency policy check passes, with existing
duplicate-version warnings. This does not establish exhaustive historical license
provenance or legal compliance for every dependency release.

## History recommendations

| Commit or group | Finding | Recommendation |
| --- | --- | --- |
| `e0e7360` → `3b1c37f` | The first commit added 428 `fuzz/target` build paths; the second deleted them. Historical refs retain 288 unique generated blobs totaling 59,191,619 B (about 56.45 MiB), including local build paths. | Remove `fuzz/target` from all history intended for public distribution, or deliberately publish a clean source snapshot. Squashing only main does not remove blobs retained by old tags/branches. |
| `8eabb2c` | The session-unlock title understates 117 changed files, including channel synchronization and logging work from an earlier line. | If curating history, reword it to cover the actual combined change; split only if the resulting commits can be tested coherently. |
| `ea017c4`, `74b3417` | Broad working-tree checkpoints mix features, documentation, CI and fixtures. | Reword by their real scope, or split during an explicitly chosen public-history cleanup. They are useful private recovery points but weak public change descriptions. |
| `8288d06`, `7ffe406`, `e6ac3b9`, `b913f88`, `5250515`, `e28b403`, `363e464` | One Vault-history deletion feature was divided into implementation, wiring, safety tests and rules. | Optional squash into one coherent feature commit with output-preservation tests. Keep the safety contract in the resulting message. |
| `72fd657`, `0772b41`, `fb3bb34` | Consecutive iterations of local launch documentation and helpers. | Optional squash. The last title's claim of dry runs for all scripts was false; the current cleanup narrows and tests the supported entry points. |
| `cf00cb8`, `30dfaa5` | A Node 24 environment override was added and immediately removed. | Optional squash, retaining the useful Actions version updates. |
| Runner branch ending at `c67954f` | Effectively incorporated by `52e8c28`; trees differ only in a status note. | No merge needed. Retire the branch only after preserving any wanted diagnostic provenance. |
| Backport `35b9f49` | Its source fix duplicates main's `2d110d5`; remaining differences are two documentation patches. | Do not merge the old baseline. Preserve it only while supporting that backport line. |
| `v0.4.4` / `2fd1031` and its performance/checkpoint ancestry | A separate historical release line remains reachable outside main. | Preserve release provenance and compare any desired fixes individually; do not merge the old tree wholesale. |
| `v0.1.0-alpha.1` | The tag suffix does not match its Cargo version `0.1.0`. | Record the historical mismatch. Do not silently move a tag that may have been distributed. Future release tags must satisfy the current version contract. |

The obsolete `master` branch is fully contained in main. No generic `fixup!`,
`squash!` or WIP titles were found. Inconsistent subject prefixes alone are not a
reason to rewrite stable history. No commit/tag signature assessment was performed.

Choose one publication strategy before changing refs: preserve the full history
with generated-path filtering, or maintain a private mirror and publish a curated
source history. Both require an explicit ref/tag migration decision. Preserve
releases already consumed by others and revalidate rewritten release points.
Nothing in this audit rewrites or deletes those refs.

## Documentation and script cleanup

- Replaced contradictory overview/status prose with the current capability and
  compatibility snapshot. Removed the superseded upload-performance proposal and
  duplicate progress diary; Git and ADR 0041 retain their history and rationale.
- Added documentation and script navigation, fixed broken anchors, disambiguated
  duplicate ADR 0035 as 0035/0035a, and corrected repository metadata to the
  configured `peppermintish/teleark` origin.
- Corrected stale schema, Keychain, recovery, transfer geometry, tuning, list-size,
  localization and signing descriptions. Kept codec tables and compatibility
  obligations rather than deleting older readers' documentation.
- Repaired the local-helper regression harness, credential scoping/permissions,
  distribution rejection of public test identifiers, exact release checksums,
  version resolution, Linux input/version selection and temporary signing/installer
  cleanup. Synthetic tests use temporary configurations and fake tools.
- Made release and package-preview jobs wait for native Windows/macOS tests, added
  script regressions to CI and removed direct ref interpolation into shell source.

All retained scripts have a build, packaging, policy or verification purpose,
documented in the [script guide](../../scripts/README.md). No script was deleted
merely because it is not a normal user entry point. Compatibility fixtures,
licenses, signing public material and native test helpers remain necessary.

## Remaining launch decisions

A monitored private vulnerability-reporting route is not yet documented. Enable
GitHub private vulnerability reporting or publish a private contact before launch.
The current [security guide](../SECURITY.md#vulnerability-reports) states this gap.

Commit `2237672` changed Telegram upload requests to 128 KiB while encoded crypto
frames remain 512 KiB. This differs from the contributor rule's transport-block
wording. Temporary `TARKUP01` bitmap records do not encode that geometry: older
multi-part records are rejected by exact-length validation, preserved under an
`.upload-corrupt` name and replaced with a fresh temporary upload identity. Small
single-request records have unchanged geometry. Lost immutable ciphertext cannot
authorize reuse; Runtime retires its encryption identity. No nonce-reuse defect was
demonstrated, but versioned temporary-progress migration and terminology need a
focused follow-up. Production transfer behavior is unchanged by this audit.

Native Windows/Linux installation, current hosted packaging/publication and
credentialed Telegram behavior were not rerun on this macOS host. Historical
hosted results are not passes for the changed scripts. The CI matrix remains the
native release qualification path; [status](../IMPLEMENTATION_STATUS.md) records
other live-system and platform limits.

## Validation of this cleanup

All inputs were synthetic or repository-owned. No personal credentials, live
Telegram account, release signing key or user files were used.

| Check | Result |
| --- | --- |
| Local-helper harness | 15 tests: 14 passed; one Linux copyright-fixture-dependent case skipped on macOS |
| Distribution credentials | 16 synthetic cases passed, including public test-ID variants and redaction |
| Release checksums | 13 cases passed, including the real child-PowerShell invocation used by Unix verifiers |
| Release version resolution | 12 synthetic cases passed, including MSI limits, tag mismatch and manual labels; actual v0.5.8 metadata resolved correctly |
| Line-ending policy and regressions | Passed; final staged index had 434 files, 391 text / 43 binary, no CRLF or forbidden scripts |
| Script syntax | All 19 PowerShell files parsed; Bash syntax and ShellCheck passed for changed shell scripts |
| Workflow validation | Both workflows passed `actionlint` |
| Linux payload / macOS library fixtures | Passed without native installation |
| Native macOS synthetic signing | Passed identity continuity, tamper rejection, PEM fallback, mismatched identity, search-list restoration and Keychain access checks |
| Repository metadata and formatting | Locked Cargo metadata and `cargo fmt --all --check` passed |
| Workspace compilation and lint | Locked all-target workspace check and strict Clippy passed |
| Workspace tests | Final serial all-target run: 889 passed, 10 existing ignored, 0 failed; separate Core (25) and i18n (26) suites also passed |
| Rust documentation | Warning-denied workspace rustdoc passed |
| Dependency policy | `cargo deny --locked check` passed advisories, bans, licenses and sources; existing duplicate-version warnings remain |
| Documentation | 295 local links/anchors resolved; executable Bash and PowerShell examples parsed without loading configuration or running installers |

Native Linux package input/version routing remains a skipped local fixture, though
its cargo-deb 2.11.1 options and staging behavior were checked against MIT-licensed
upstream sources linked in the script guide. Windows ACL/installer execution,
macOS installer execution, Linux native packaging and hosted workflows require
matching disposable runners. No UI layout or application transfer behavior changed.

The first workspace test attempt stopped in a GUI test without an assertion or
panic diagnostic. That exact binary passed the focused test and all 189 GUI tests
on a diagnostic rerun. A separate follow-up compilation then failed with an
explicit no-space-left error. Removing only the regenerable local
`target/debug/incremental` cache restored working space; the final complete
workspace run and warning-denied rustdoc passed. No application source was changed.
Disk pressure is a possible explanation for the first interruption, not a proven
cause. The failed attempts are not counted as successful runs.
