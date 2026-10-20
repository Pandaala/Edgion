# WAF profile capture-hit indexing

Status: deferred indexing and budget decision for the user.
Source baseline: `fd537a8d9`. No runtime or dependency change.

## Evidence

Gateway structured request-body inspection reaches JsonCollector and its
ProfileObserver::stage in edgion-waf/src/seclang/json_inspection_collector.rs.
In Accelerated mode with a nonempty domain capture-hit plan, each emitted
transformation stage iterates every batch in that JSON/XML domain. It charges
one work unit before testing batch.profile against the current profile. Matching
batches then validate their peer range, charge peer work and recompute eligibility.
Reference mode bypasses this optional preparation.

The scan therefore includes batches belonging to other profiles, even when none
belong to the current profile. A profile-specific index could avoid those visits.
This is a source-level opportunity, not evidence that the scan dominates a real
workload. The earlier constructor-capacity optimization removes a separate scan
once per inspection; it does not change this per-stage loop.

capture_hit_plan::install_domain records stable batch indexes in HitBinding and
appends name/value batches for each monotonically allocated matcher. Matcher
order does not guarantee contiguous ranges by profile. Sorting batches in place
without rebuilding bindings would invalidate their references. An auxiliary
index adds generation metadata, compiler work and bounded allocation accounting.

## Decision

Keep the current scan, add bounded per-profile batch references/ranges, or redesign
the canonical layout while maintaining every binding. Measure sparse and dense
profile participation, batch counts and emitted stage counts before choosing.
Compare generation retention and compilation peak with saved request work; retain
optional compilation's fallback when its extra metadata cannot be admitted.

Choose the intended work-budget contract explicitly. Dropping unrelated batches'
charges changes near-limit acceptance. Charging their total at stage entry moves
failures ahead of the current interleaved peer validation and peer charges.
A design retaining the original logical charges must preserve relevant ordering,
partial usage and sticky failure behavior, or explicitly document the approved
change. An index alone is not proof of accounting-neutral behavior.

Eligibility remains stage-specific: a peer can become terminal after an earlier
multiMatch stage, and epoch wrap must clear old selected-peer evidence. Do not
cache eligibility across stages merely because the profile and batch are static.
Original capture extraction, negation, Reference execution and replay own their
existing semantics; the optional hit matcher only supplies a valid boolean proof.

## Acceptance for an approved implementation

Compare with the original ordered scan using several profiles, multiple matchers,
shared name/value matchers, JSON and XML, profiles without matching batches, empty
optional plans and sparse/dense dispatch. Preserve canonical batch bindings,
terminal-peer recomputation and epoch-wrap behavior. Cover both evaluation modes,
whole/chunked inputs and emitted multiMatch stages rather than only final matches.

Exercise exact and one-below work and physical limits, including refusal between
matching and nonmatching batches, peer-range validation, scratch reuse and
optional compiler refusal. Verify failure identity/order and retained usage as
well as selected outcomes, captures and actions. Existing tests establish current
behavior; they do not validate an unimplemented index or quantify its benefit.

Keep one canonical bounded inspection owner. Add no body cache, response
aggregation, request tracing, payload logging or metrics. Gateway remains the
anchor; a shared-leaf implementation needs a frozen supporting-file allowlist
and independent IMPL plan/final reviews. Keep integration fixtures unchanged.
This task records choices for the user and does not authorize implementation.

## Knowledge and verification limits

See [fused JSON inspection](../../skills/04-review/waf/regression-guards/fused-json-inspection.md),
especially optional capture-hit batches and stage-cache membership storage.
Existing collector regressions cover terminal multiMatch peers, epoch wrap,
inline/overflow namespaces and physical admission. No alternative implementation
or benchmark accompanies this task.
