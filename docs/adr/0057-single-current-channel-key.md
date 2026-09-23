# ADR 0057: Select one current key for the managed channel

Status: Accepted — 2026-09-24. Supersedes the multi-key desktop behavior in [ADR 0025](0025-fixed-channel-and-retained-key-epochs.md) and the recovery controls in [ADR 0040](0040-automatic-device-keys-and-optional-pin.md). The earlier documents remain historical records; their cryptographic format and channel ownership requirements still apply.

## Decision

When an account's managed-channel catalog becomes ready, Runtime checks readable credentials in the **selected** storage backend: private SQLite when Keychain is off, or the system Keychain when it is on. It authenticates channel manifests with candidate keys off the UI thread and chooses one key that opens the channel's files. That key alone is the session and upload key. Stored former keys do not remain simultaneously usable in the session. An empty channel can select a readable stored key without a manifest to authenticate.

On first login without a managed channel, remote discovery creates and verifies one private channel. Setup durably marks that channel's key setup pending before saving its binding, then generates a fresh key before reporting success, even if an older channel left a stored credential. The storage timeline includes key creation. A later login may finish interrupted key setup only while that local marker still names the bound channel and the active key epoch has not advanced. It checks the synchronized managed catalog and fresh remote manifest and pending-upload locators before generating. Other discovered empty channels are never provisioned automatically. A channel with managed files continues to show the missing-key or undecryptable helper instead of silently changing keys. Account and Vault session revisions fence delayed setup from a subsequent login; a stale key operation cannot publish its record or key into the new session.

The result is `Ready`, `NoKeys`, or `Undecryptable`. `NoKeys` means the selected backend has no stored credential material. `Undecryptable` means stored keys exist but none authenticate the managed channel's discovered files. Files shows only the corresponding helper until `Ready`; Raw Files continues to show actual Telegram objects. The choice is tied to account and channel generations, and late completion cannot publish a key for another session.

Settings offers key import and, only for `Ready`, export of the selected key. Import does not by itself prove channel access; it triggers selection again. Switching the configured credential backend also triggers selection again. The export API also checks `Ready`, so a hidden control cannot bypass the rule. Automatic channel events trigger catalog revalidation and rescan, including after bounded event history is evicted. Protocol reconciliation and a bounded quiet fallback cover missed remote notifications.

This is an intentionally breaking desktop behavior change. Previously supported simultaneous historical-key browsing, rotation controls and explicit new upload epochs are no longer promised. Existing encrypted bytes and stored credentials are preserved; no data reset or silent deletion is part of selection. A channel containing files that require different keys remains unavailable in Files until a single usable key can authenticate the discovered files. Users can still inspect the underlying objects in Raw Files.

## Evidence

Runtime selection tests cover no credentials, matching and mismatched keys, multiple stored epochs, and the selected backend. GUI tests cover Files warning and Raw Files at compact and full-screen sizes. The event-feed regression proves managed changes remain detectable after bounded delta eviction. Live Telegram credentials are outside these synthetic checks.
