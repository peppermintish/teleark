# ADR 0055: Stable self-signed macOS release identity

Status: accepted, 2026-09-22

Self-signed code signing establishes continuity between TeleArk releases for macOS Keychain access. A new identity every build, ad-hoc signing, or unsigned code cannot provide that continuity. We generate one RSA-3072 identity, commit only its public certificate, and retain the private key outside the checkout and in the repository Actions secret store.

App and standalone executable use `app.teleark.desktop` and a designated requirement pinned to that certificate. Packaging fails closed when the identity is missing or mismatched. Each signing run imports the PKCS#12 identity into a disposable private Keychain as one identity, verifies the pinned public fingerprint among all certificate/private-key identities before applying the restricted partition list, verifies both outputs, and removes the temporary keychain and key files. Older macOS/OpenSSL combinations may use the explicitly typed PEM fallback after the same identity check. The identity lookup must not use the valid-only filter: system certificate trust is independent of this pinned self-signed identity. Successful signing and signature verification remain required. No blanket Keychain ACL or system-root trust change is used. Installer payload verification handles the distinct standalone and bundle signatures.

This reduces repeated prompts for items created by the same signed identity. It cannot grant access to another identity’s items, unlock a user’s Keychain, or provide Apple notarization/Gatekeeper trust. Runtime uses noninteractive access and actionable failure instead. Existing recovery material is retained on failure. Certificate rotation must include continuity/recovery planning and cannot silently replace the pinned identity.

Evidence: native synthetic signing test signs two resource versions with the same designated requirement, verifies standalone code equivalence and rejects modified resources. Full release workflow verifies both universal slices and installer upgrades.

Primary references: [Apple code signature guide](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/AboutCS/AboutCS.html) and [TN3127 requirements](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements).
