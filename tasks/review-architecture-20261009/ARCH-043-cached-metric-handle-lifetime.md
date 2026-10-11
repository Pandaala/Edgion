---
id: ARCH-043
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-285
analyst: /root/verify_ip_union
independent_verification: confirmed-with-revision
---

# Give cached Gateway request metrics an eviction-safe lifetime

## Current ownership and ordinary trigger

Gateway startup installs the shared recorder before resource watches. Metrics API
passes request_duration.idle_timeout_secs even when the optional duration metric
is disabled. The default is 600 seconds; Common enables MetricKindMask::ALL.
GatewayMetrics retains four eager Counter/Gauge handles and nine lazy OnceLock
Counters. Immutable HTTPRoute/GRPCRoute owners retain backend Counter matrices
and enabled duration Histograms. pg_logging writes through them without renewed
registration. The sole Gateway render boundary protects LB/Wasm handles, but
not these request handles.

After an ordinary non-test routed request, periodic scrapes with no updates
beyond the idle interval delete its registry entry. A later request through the
unchanged route matrix increments the retained old Arc. GatewayMatrixEntry::record
returns true because its slots exist, suppressing dynamic fallback. The exporter
no longer visits that Arc. Another emitter registering the same key creates a
new handle and does not reconnect this old one. Permanent GatewayMetrics handles
and cached Histograms have the same mismatch. The locked exporter 0.18.3 and
metrics-util 0.20.4 render deletion/retained storage establish this statically;
no incident, measurement or executed reproduction is claimed.

## Bounded improvement and alternatives

Give retained writable handles an explicit lifecycle at the existing canonical
registration/render boundary. Counter/Gauge zero increments immediately before
synchronized render follow the actual LB/Wasm precedent. Current and in-flight
old route owners need cold ownership registration; final release must stop
pinning without request blocking. Registration must not race render into an
already detached initial handle. Process eager handles are fixed and small.
Preserve absent-until-first-use cold families and eviction of unowned/dynamic
series. Existing direct registration for rare events is a bounded alternative
to permanent OnceLocks. The original rejection of a cold_counter macro concerns
style, not acceptance of detached writable storage.

Histogram record(0) corrupts samples; record_many(value,0) does not advance the
locked implementation's generation. Neither is a keepalive. One scoped
alternative retains Counter fast paths and renews optional duration handles
through the existing dynamic record_request_duration registration. This prevents
permanent cached-handle detachment in sequential use, but is not by itself an
exactly-once concurrency guarantee: the renderer snapshots a generation and
subsequently deletes by key without revalidating that generation, so registration
or sample recording can race eviction. If this alternative is selected, its
enabled-only registration/sample boundary must be reviewed against rendering and
must preserve real samples through that race. Account for its label/registration
and synchronization costs, first-event behavior, disabled zero work and
exactly-once samples. Sample-free exporter pinning is another alternative requiring
a separately reviewed seam. No dependency redesign, concrete pinning API or new
request-blocking mechanism is approved by this finding.

## Invariants, cost and future checks

Preserve names/units/labels, final downstream status and route/backend attribution,
integration isolation, cardinality guard admission/no-op behavior, binary-owned
recorder setup and unrelated cleanup, LB/Wasm lifecycle and bounded scrape-time
cert sampling. No new metrics, request values, IP/generation labels, exporter,
timer, registry scan, mock-driven blocking lookup, globally disabled eviction,
permanently retained deleted routes or artificial samples. Keep request detail
in canonical access logs. No throughput or RSS benefit is claimed.

Existing route tests cover registration/bounds/sparse slots/atomic fallback/label
parity; eager/cold/gauge tests omit eviction. Actual LB/Wasm tests cover their own
families; the original LB guard explicitly does not certify other retained
Gateway handles. Future actual-exporter tests must record/render/expire/re-record
through unchanged production owners and render again: permanent and initialized
cold handles, matrices, overlapping owners/final release, evictable unpinned
control, rejected no-op handles, no fallback double count. Enabled histogram
count/sum/buckets must be exact so fake keepalive samples cannot pass. Include a
controlled registration/recording-versus-eviction interleaving for the selected
histogram design; sequential renewal alone cannot establish that contract.

The metrics inventory idle table says default 0/no-op, stale against actual600s
ALL-kind setup. Record a scoped documentation follow-up; do not use it to reject
the source witness. Historical route design path in a module comment is absent.
One shared root cause spans registry.rs, route_handles.rs and api.rs; no duplicate
cold macro/file-size issue. Analyst coverage includes the five metrics primaries.
Independent verification inspected the affected registration/render and recording
implementations, actual pg_logging/cold publication, Common installer/cardinality
guards, original observability/LB/Wasm contracts and actual test/dependency bodies.
Static read-only analysis only; no tests/builds/network/Git/mutations. Independently
confirmed with the histogram concurrency qualification above.
