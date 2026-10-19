# Evaluate bounded endpoint probe projection

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `266f6799e`.

## Evidence and decision

AllEndpointStatus::resolve_endpoints scans a published Service section's ready
slots and materializes a ResolvedEndpoint for every address accepted by the
canonical safety validator. Each record owns formatted address and Service-name
Strings and a transport Arc. collect_route_endpoints_with receives this complete
Vec and only then keeps the remaining global prefix with take(remaining).
Later Services are still resolved when that remaining capacity is zero.

The selected fan-out is bounded by effective_max_endpoints (at most 50), but the
temporary projection Vec and its owned strings scale with the Service's safe
ready slots. The process concurrency limit of three and configurable interval
gate constrain invocation frequency/concurrency; they do not bound this per-Service
projection by the selected prefix. This is a source-level cost observation, not
a measured production bottleneck, memory exhaustion or attack reproduction.

Evaluate separating full validation/counting from owned record construction:
continue scanning every relevant slot, but materialize only the remaining selected
prefix. A remaining-capacity resolver or a bounded visitor could express that
contract. Choose the representation only after comparing preparation allocations,
CPU and complexity for realistic Service sizes. A new endpoint cache or shared
client pool is not part of this proposal. Full scans may remain necessary for
existing accounting even if record allocation becomes bounded.

## Required boundaries

Preserve per-backend safe endpoint_count, saturating discovered/unsafe totals,
truncated, global prefix order, duplicate-address ordinal identity and backend
result order. A full prefix must not suppress later missing Services, invalid
ports or unavailable sections: invalid_ref_count still causes the existing 503.
Denied references and non-Service kinds retain their existing filtering rules.
Validate unsafe addresses throughout the scan, including after the selected cap.

Preserve captured discovery/policy snapshots, section lookup, each slot's proven
serving port and transport construction semantics. Invalid TLS policy is a probe
failure for selected endpoints, not an invalid-ref 503. Existing generation checks
run during selected-probe preparation; do not invent checks for unselected slots
or move them to a different boundary. Keep canonical TLS/SSRF enforcement,
per-invocation client isolation, permits and rate/global-concurrency ownership.

Keep collection before the fan-out deadline and retain current timeout, latency,
partial-result and error contracts. The separate endpoint-01 task concerns address
copies within fan-out/result assembly; it does not authorize those changes here.
No new configuration, schema, logging or metrics are needed.

## Acceptance for a later implementation

Compare temporary allocation calls/bytes and preparation CPU for Services below
and far above the selected cap, multiple Services, exhausted capacity, duplicates,
and mixed safe/unsafe slots. Report both temporary record storage and the remaining
full scan; do not claim all request memory becomes bounded by maxEndpoints.

Exercise full counts with zero selection capacity, exact/over-cap prefixes,
unsafe slots before/after the cap, later invalid references, denied references,
IPv4/IPv6 serving ports, snapshot replacement and selected TLS-policy failures.
Use the actual discovery resolver as well as pure collection tests. Assert no
record materialization beyond the selected prefix without weakening validation
or hiding later errors. Run IMPL plan/final reviews and repository gates.

Fresh all_endpoint_status selection passed 23 tests on the current implementation.
No candidate implementation, large-Service allocation benchmark or live probe ran.
Canonical authority: skills/04-review/backends/regression-guards/all-endpoint-status-tls-identity.md.
