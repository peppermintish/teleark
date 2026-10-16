# TeleArk grammers-mtsender patch

Upstream: grammers-mtsender 0.11.0 from crates.io, archive SHA-256 `b009d7d0f571cabac15e1ac7f588fefb764f23aba13bf8b9063a0a32a036075d`.
License: MIT OR Apache-2.0; original headers and both license texts are retained.

Only `src/net/tcp.rs` differs from the published implementation: parse numeric
IPv4/IPv6 proxy hosts as IP addresses before constructing a DNS resolver; map
resolver initialization failures to I/O errors instead of panicking. SOCKS URLs
are non-special URLs, so the URL parser may classify numeric IPv4 as Domain.
The application's authenticated IPv4 loopback gateway must work without DNS.
No proxy bypass, IPv6 requirement, DNS configuration change or direct fallback
is introduced. Real sender-pool bootstrap/reconnect tests live in
teleark-telegram/src/network/tests.rs.

Remove this patch when a published upstream release provides equivalent
behavior, then rerun every proxy routing and isolation regression. Cargo's
normalized manifest and upstream tests/docs are preserved.
