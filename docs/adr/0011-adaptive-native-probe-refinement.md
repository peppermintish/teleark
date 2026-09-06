# ADR 0011: Refine native throughput probes on both sides of a measured peak

Status: accepted
Date: 2026-09-06
Supersedes: ADR 0010 numerical ramp and probe-refinement policy only

## Context

A fixed +16 probe can skip safe intermediate concurrency, and starting at P16
assumes more capacity than weak networks provide. A successful coarse probe can
also pass a smooth throughput peak without immediately causing errors. Old
bounds become unreliable when capacity changes mid-transfer.

## Decision

Max Throughput starts at P4, searches P1–64, and initially grows by at most its
current P and 16. Strong gains settle in one second; weak gains get a fresh
five-second measurement before rollback. Fine upward probes need 1% gain and
coarse probes 3%. Failed probes narrow an upper interval to single-part precision.
At its boundary the controller also checks the lower interval, so it searches
for useful throughput rather than just the largest non-failing P. Downward
probes wait five seconds and undo losses greater than 0.5%.

Retry bursts without a pending upward probe halve P; downward tests can reopen
old lower bounds after major rate collapse. Adjacent upper boundaries and
completed lower searches are revisited on a 60-second cadence. The existing
bounded retry budget, cooldown, memory guard, and shared server-wait gate remain
mandatory. Preference, checkpoint, and session-log encodings do not change.

## Validation and limitations

Deterministic capacity models cover every integral retry boundary from 1 to 64,
rate optima from 10 to 20 without errors, smooth peaks between successful coarse
probes, plateau/noise, growing/shrinking capacity and truthful physical bounds.
They are policy regression tests, not Telegram bandwidth benchmarks. The
controller optimizes the measurements it can observe and cannot guarantee a
global optimum in arbitrarily varying networks, exclusive LAN bandwidth, or
absence of server restrictions. Credentialed throughput measurement and the
previously documented UI visual checks remain pending.
