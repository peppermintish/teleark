# ADR 0010: Separate native throughput strategy from soft-limit policy

Status: accepted
Date: 2026-09-06

## Context

The native P4–24 envelope was selected from one set of measurements. Other
networks benefited from the former P64 ceiling. AdaptiveOverride only governs
advisory active-file limits and cannot select the native concurrency envelope.

## Decision

Add `preferences.v1.download_throughput_strategy`, canonically `balanced` or
`max_throughput`. Missing keys retain Balanced. This is an additive version-1
setting; unknown suffixes remain ignored by older readers, while invalid known
values fail with a structured persistence error. Literal-row compatibility and
save/reopen tests accompany the change. No database schema, checkpoint, part-map
or session-log schema changes are needed.

Max Throughput uses P16–64 with fast measured probes, greater tolerance for
short-lived rate noise, and eight bounded part attempts. Network retry delays
are scheduled per part so healthy completions continue. Mandatory server waits
use a shared native-download gate retained by the Telegram connection. Neither
strategy creates additional authorization sessions or changes RPC size limits.

The runtime captures the preference once per native task start/resume/retry.
Settings exposes it separately from the soft-limit controls in all three locales.
The encrypted Vault adapter remains unchanged because its actual concurrency
capabilities differ.

## Consequences

Max Throughput may consume more bandwidth/memory and incur more recoverable
errors. It cannot guarantee LAN exclusivity, a particular rate, or absence of
server restrictions. Balanced remains the compatibility default. Real throughput
requires credentialed measurement. Server wait gates are in-memory, survive
native task changes within the connection, and do not coordinate process restarts
or independent connections. Protocol guidance comes from the official Telegram
[files](https://core.telegram.org/api/files) and
[errors](https://core.telegram.org/api/errors) documentation; no external
implementation or new dependency is introduced.

## Probe refinement

The numerical probe policy now narrows its step near measured boundaries.
Initial steps up to 16 are retained only with sustained useful throughput gains;
low gain halves the step. Retry bursts or confirmed throughput regression record
an exclusive failed-P bound, restore the prior value, and search within the
remaining interval down to one-part precision. Learned boundaries are rechecked
no more frequently than 60 seconds after a failure, at +1. This is a reversible
controller tuning change; the preference, session-log and checkpoint encodings
are unchanged. Deterministic tests cover intermediate limits and changing capacity.

ADR 0011 supersedes the numerical ramp/refinement policy above; the additive
preference encoding and transport protections remain unchanged.
