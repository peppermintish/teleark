# ADR 0007: Adopt Dual Permissive Licensing and a GPL Clean-Room Policy

- Status: Accepted
- Date: 2026-08-24

## Context

TeleArk is intended for permissive distribution. The Go project `tdl` is GPL-licensed and is therefore unsuitable as an implementation reference. Copying, translating, adapting, or reproducing its implementation details could compromise clean provenance. Dependencies and bundled assets can also introduce reciprocal terms or attribution obligations, including through upstream Git branches whose graphs differ from published releases.

## Decision

License TeleArk under `MIT OR Apache-2.0`, with `LICENSE-MIT` and `LICENSE-APACHE`, and understand contributions to be offered under the same terms unless explicitly stated otherwise.

Apply a strict clean-room rule: contributors and agents must not inspect, copy, translate, port, adapt, derive from, or use implementation details from GPL `tdl`, including its source, tests, architecture, schemas, state machines, naming, control flow, or Telegram RPC sequencing. They may not ask another person/agent to inspect it and summarize it. Use official Telegram/MTProto documentation, public `grammers` APIs/docs, permissively licensed crate documentation, public specifications, and original TeleArk analysis/tests.

Review each dependency's direct/transitive licenses, maintenance, security posture, and necessity before adoption. The GUI dependency baseline selected by upstream research is pinned to published crates.io releases:

```text
gpui = "=0.2.2"
gpui-component = "=0.5.1"
gpui-component-assets = "=0.5.1"  # optional if its bundled assets are needed
```

Do not substitute the current Zed/git-main dependency path without a fresh legal audit: the graph reviewed for this decision pulled `ztracing`/`zlog` under `GPL-3.0-or-later`, which is incompatible with the intended permissive distribution policy.

When Lucide icons are distributed through an assets package, preserve the Lucide ISC and applicable Feather MIT copyright/license attribution in `THIRD_PARTY_NOTICES.md`. Audit the exact bundled asset inventory and licenses; Lucide's terms do not automatically cover every neighboring asset, trademark, or font.

The locked published GPUI graph also declares MPL-2.0 for `cbindgen`, `dwrote`, and `option-ext`. These are recorded as compatible build/platform/helper exceptions, not described as permissive licenses. A production release must retain required notices/source-offer obligations for any shipped covered files and enforce a reviewed dependency policy.

## Consequences

- Repository-owned code has clear dual-license terms and patent protection is available through Apache-2.0 at the recipient's option.
- GPL/AGPL/incompatible/unclear source is not an implementation reference, even if studying it would be convenient.
- Dependency upgrades, git revisions, features, and asset bundles require renewed license review; a package name alone is not sufficient evidence.
- Exact GUI release pins trade access to upstream changes for a reviewed dependency graph; upgrades must be deliberate and tested.
- Distribution includes the maintained Lucide/Feather asset notice; an exhaustive generated dependency notice is still required before production release.
- CI enforces the repository's reviewed advisory, license, source, and ban policy with `cargo-deny`; dependency graph and policy changes are reviewed together.
- If a required capability has only incompatible implementations, TeleArk must design it independently from authoritative specifications or seek legal review, not rewrite GPL code superficially.
