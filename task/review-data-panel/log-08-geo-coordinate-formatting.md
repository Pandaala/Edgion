# Evaluate GeoIP coordinate formatting in access-log snapshots

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `0ef92540f`. No runtime or masking-policy change.

## Evidence

`GeoSlotLog::from_lookup` in `edgion-gateway/src/observe/access_log/entry.rs`
borrows geographic dimensions but constructs `format!("{lat},{lon}")` whenever
a projected Found slot has both coordinates. Each remote/direct-peer slot can
create its own intermediate String. Missing coordinates omit the field.

The field uses the canonical `edgion_resources::sensitive::masked_opt` serializer.
It reaches `apply_sensitive`, which reads the process-global mode during
serialization. Replace emits the replacement text, so the previously formatted
coordinate is unused in that output. Off emits the coordinate and Wrap surrounds
the complete string through the shared masking implementation. This is evidence
of intermediate formatting, not a measured latency or throughput problem.

Terminal logging and optional connection-established snapshots independently
construct AccessLogEntry. DebugAccessLogToHeader also constructs a snapshot when
its existing Wrap suppression does not apply. Actual coordinate availability and
snapshot frequency determine the cost; not every request or callback pays it.
Logging reads only already-resolved geo slots and must not trigger a lookup.

## Decision and acceptance

Measure whether the intermediate string warrants a change. Options include
retaining the current representation, formatting into a proven bounded temporary
buffer, or using a lazy coordinate projection through a reviewed shared masking
API. The current canonical API requires contiguous text. Any formatter-aware
extension belongs to that canonical owner; coordinate-local masking is not an
acceptable shortcut. Coordinate-specific evidence complements
[log-01](log-01-raw-uri-fragment-serialization.md), which owns the related raw-URI
fragment/masking design question; this task does not approve that wider change.

Do not skip formatting based on mode sampled at entry construction: current mode
selection occurs at serialization. Preserve Off, Replace and Wrap for the whole
coordinate string, replacement/wrapper configuration, JSON escaping, error
propagation and AccessLogEntry fallback. Preserve the existing floating-point
Display byte representation, including signed zero, extremes and nonfinite values
where current GeoInfo inputs permit them. Do not silently normalize precision,
switch the JSON field to numeric coordinates, or assume an unproven stack bound.

Retain independent slot status/omission rules, partial-coordinate omission,
borrowed dimensions, lazy geo-state allocation and captured facts across provider
reload. Serialize within the context lifetime and retain the final owned JSON for
sinks/headers. Do not add an eager coordinate cache to GeoInfo or RequestInfo,
force GeoIP lookup, introduce another buffer pool or alter debug Wrap suppression.

Compare actual JSON bytes and serializer failures under all modes, including mode
selection between construction and serialization. Measure projection and complete
serialization separately for absent, partial, one-slot and two-slot coordinates;
report allocation calls/bytes, retained state and CPU without claiming throughput
from allocation counts alone. Repeated snapshots and the final output allocation
belong in the cost model. Shared Wrap-buffer first-use/growth and non-reentrancy
remain explicit constraints, not a blanket zero-allocation guarantee.

Original rules: `skills/03-coding/observability/00-access-log.md` and
`skills/04-review/observability/regression-guards/geoip-no-global-enabled-switch-access-log-lazy.md`.
Any implementation requires a separate IMPL plan, exact source allow-list and
independent review of canonical masking changes.

Fresh unchanged-code verification passed 18 access-log entry tests, formatting
check, workspace all-target check/Clippy and agent-doc validation. Existing
unrelated warnings remain. The coordinate test checks presence/omission, not
complete mode equivalence; these results do not establish the proposed acceptance.
No new tests, full suite, live integration or performance benchmark ran.
