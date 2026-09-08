# ADR 0024: One fail-closed application network route

Status: accepted, 2026-09-08

## Decision

Settings supports SOCKS5 and HTTP CONNECT, numeric IPv4/IPv6 endpoints and optional authentication. The route is explicit and immutable for one retained Telegram runtime generation. Every sender pool, datacenter, authorization exchange, reconnect, upload, download and avatar request uses the same authenticated loopback SOCKS5 gateway. Its sole outbound dial selects either the configured proxy endpoint or, only for an explicitly direct policy, the Telegram destination. Proxy failure never changes that selection. Environment proxy/bypass variables have no authority. Domain endpoints/targets are rejected to avoid a separate DNS exit.

The gateway is bounded to 32 tunnels with a random 256-bit local credential, 10-second connection/handshake deadlines and 8 KiB HTTP response headers. Directional copy buffers are bounded; EOF closes the tunnel and publishes failure. It adds one local TCP hop, including in direct mode; no end-to-end throughput improvement is claimed. The protocol implementations follow RFC 1928/1929 and RFC 9110 CONNECT semantics using public permissively licensed grammers APIs.

Applying a route closes admission, advances the generation, cancels and joins the entire old runtime and all socket tasks, persists the policy, and only then creates its replacement. This work runs off the GUI. No lock spans joins or persistence. A failed save/start leaves the endpoint unavailable, with no reconstruction of an old direct endpoint. Stale requests and observations cannot affect the new generation. Existing recoverable transfers retain their durable state; active network requests may fail during the switch and use their ordinary retry/resume controls.

The framework HTTP client is blocked so remote assets cannot create an independent connection. External API help links copy to the clipboard under a proxy policy (including a pending proxy draft or failed policy load), because a browser is outside this route's control. This is application enforcement, not an operating-system firewall.

## Persistence and feedback

Schema 14 creates the explicit initial direct policy for earlier databases. This also fences older schema readers that would otherwise ignore a saved proxy. The independent `network.proxy` JSON codec is version 1; missing, malformed or newer policy bytes block network startup and cannot be overwritten by normal save. See [format](../PREFERENCES_FORMAT.md#network-proxy-policy-version-1).

A retained revisioned monitor publishes a bounded 64-event timeline, omissions, phase duration and last network-event age. Applying, connecting, queued tests, testing and blocked outcomes remain globally visible across navigation. Failures remain sticky until explicit apply/test acknowledgment. No byte throughput or end-to-end Telegram health is inferred from a successful tunnel probe. The probe has a reserved dispatcher lane and cancellation, so occupied read/transfer lanes cannot starve diagnostics. Cancellation retains the proxy policy.

## Evidence and scope

Deterministic tests use real loopback proxy peers and reachable destination traps: payloads, raw authentication, both address families, refusal, redirects, malformed/oversized replies, dropped sockets, timeouts, retries and environment bypass attempts. Real grammers pools cover multiple datacenters/reconnections and plaintext/authenticated MTProto bootstrap bytes. Runtime tests retain old root/child sockets to prove the apply barrier closes them, and cover persistence failure, restart, stale observations, cancellation and reserved probe admission. Architecture guards restrict network construction and an actual blocked framework HTTP request verifies no socket is opened. GUI tests cover persistent failure/waiting feedback and editor states.

These tests do not emulate a complete Telegram server or authenticate a real account. Protected real-account interoperability and full assistive-technology/platform qualification remain separate evidence.
