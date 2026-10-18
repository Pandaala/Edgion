# Endpoint probe result address ownership

Status: deferred for user decision. No runtime change.
Source baseline: bac052bc6.

## Evidence and scope

In AllEndpointStatus's `fan_out_requests`, each selected endpoint address is
cloned while collecting the local FuturesUnordered. The clone precedes permit
acquisition, the per-endpoint start timestamp and the wall deadline constructed
by `fan_out_with_wall_timeout`. Completed futures transfer that String into their
result. When the wall timeout discards unfinished futures, their address copies
are discarded too; `assemble_endpoint_results` copies the selected address again
for each missing ordinal's timeout result.

Borrowing the address until a result is produced would avoid those wasted copies.
The local future lifetime permits borrowing, but moving allocation into the
completion branch also moves it inside the wall-timeout budget and permit-holding
interval. It could affect which peers finish near the wall deadline. Sampling
endpoint latency before copying preserves that individual measurement boundary,
but does not preserve the wall deadline boundary. This is source-level reasoning,
not an executed timing regression or measured throughput result.

## Decision requested

Choose whether to retain the current implementation, authorize investigation of
the deferred-copy timing tradeoff, or design result-address materialization after
fan-out. The latter can use the existing ordinal and selected endpoint owner to
attach addresses during assembly, but changes private result representation and
requires separate value/plan review. Do not silently expand this into endpoint
identity, TLS client sharing, concurrency or timeout policy changes.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Trace successful, preparation-error,
transport-error, pending-timeout and externally cancelled futures. Measure scoped
address allocation counts, and distinguish final owned output from temporary
copies. A successful-call copy reduction or overall memory benefit is not proven.

Preserve duplicate-address ordinal identity, selected-prefix order, status/error
text, raw body-byte accounting, timeout markers and partial result assembly.
Verify permit lifetime and ready-at-deadline selection using deterministic tests.
Existing helper tests cover partial completion and assembly but do not establish
real network deadline equivalence. Retain generation checks, endpoint pinning,
TLS identity, SSRF guards and bounded response reads. State any authorized timing
boundary change explicitly; do not infer a throughput benefit from allocations.
