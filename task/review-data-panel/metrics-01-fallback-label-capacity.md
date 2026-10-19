# Evaluate dynamic fallback metric label capacity

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `9d465168f`. No runtime change.

## Evidence

`pg_logging::record_request_metrics` tries the route's prepared metric handles
before `record_request_metrics_fallback`. Integration-testing mode, missing route,
gateway or backend identity, unknown protocol and handle/index mismatch can reach
the fallback. This proposal does not apply to every request.

In `observe/metrics/registry.rs`, `record_backend_request` reserves ten Label
slots on every call. The default configuration emits four: gateway namespace,
gateway name, protocol and status. Optional route/backend pairs and nonempty
integration test labels can fill the ten slots. Even all-disabled labels with
empty test fields currently reserve a vector.

`record_request_duration` returns before allocation when disabled, as it is by
default. When enabled, it reserves eight Label slots. Its default label mask
emits four: route name, backend name, protocol and status. All eight can be
selected independently. Static status-label construction is already implemented;
this task concerns vector storage, not that removed String construction.

Locked metrics 0.24.6 moves a Vec through IntoLabels unchanged. Key::from_parts
passes it to Cow::from_owned, whose slice implementation preserves length and
capacity in owned_into_parts. There is no automatic shrink at this boundary.
This does not establish how long spare capacity survives later key clones or
recorder processing, or prove a retained-memory or throughput improvement.

## Decision and acceptance

Choose whether measured benefit justifies exact per-call capacity, a prepared
configuration-owned count, or retaining the fixed bounded reservation. Exact
counting repeats flag work before building the vector; prepared counts must stay
consistent with every initialization and test construction path and still account
for optional test labels. A stack/small-vector staging buffer still needs an owned
Vec for this dependency's IntoLabels API and can add copying/conversion work.
Do not introduce descriptor caching as an incidental fix: that needs separate
canonical dynamic-admission, expiry, renewal and configuration-lifetime review.

Measure fallback frequency and zero/default/full label masks, empty/nonempty test
labels, enabled/disabled histogram and ordinary handle hits. Compare requested
allocation bytes, allocation/reallocation counts and CPU cost through the current
canonical facade, distinguishing descriptor construction from registration and
renewal. No candidate implementation or benchmark has been run. Fewer requested
slots alone does not prove fewer allocations or faster requests.

A future IMPL must preserve metric names/kinds, exact values and label ordering,
optional-label omission, histogram gating, counter increments, histogram samples,
route-handle/fallback parity and dynamic admission/expiry behavior. Add no new
labels, exporter handles, global caches, tracing or high-cardinality diagnostics.
Revalidate the locked dependency ownership path before implementing. Review a
bounded allow-list and test all label-mask combinations plus optional test fields;
retain canonical catalog and inventory checks.

Authority: `skills/03-coding/observability/01-metrics.md` and the Common metrics
facade/catalog. Fresh unchanged-code verification passed 36 metrics and ten
terminal-logging tests (46 total), formatting check, workspace check/Clippy and
agent-doc validation. Existing unrelated warnings remain. No full test suite,
live integration or performance measurement ran.
