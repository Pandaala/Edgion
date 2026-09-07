---
name: degrade-selection-counter-ownership
description: Use when reviewing Service degrade observability, cached metric ownership, lifecycle exits, or cross-section aggregation.
knowledge_type: regression-guards
---

# Service degrade observability counts selection events

## Conclusion and scope

The Gateway exports `edgion_lb_degrade_selections_total{service}` as a Counter.
One event means `select_service_endpoint` entered the degrade branch for one
selection. Retries and different Service port sections count separately; an
event does not establish successful forwarding. This is neither a current-state
gauge nor a unique-request outcome counter.

The user explicitly approved replacing the old gauge after comparing Envoy and
Kong. The original task `lb-degrade-mode-metric-lifecycle` required gauge reset
coverage; that proposed contract was superseded by the event-counter decision.
The original defect was real, not an accepted stale-state behavior.

## Rationale and evidence

Previously `degrade_engaged` set a Service gauge only after survivor evaluation.
Threshold disable, health/outlier capability removal, empty selectors, and
Service deletion could leave a previous one exported indefinitely. Additionally,
sections of one Service wrote different decisions into the same gauge, so a
healthy port could hide another port's degrade state. A normal selection after
a short degrade interval could also erase all evidence before the next scrape.

- `GatewayMetrics::lb_degrade_selections` owns registration in the canonical
  metrics registry. Labels contain only a canonical `namespace/name` Service key.
- `ServiceRuntimeSnapshot::with_degrade_counter` registers on the cold publication
  path only when threshold > 0 and health or outlier policy is present. Snapshots
  reuse a prior handle across policy and membership changes, including disable.
- `select_service_endpoint` uses its existing `degrade_engaged(selector, active)`
  result and the same snapshot's Counter. There is no additional configuration
  read, label construction, or metric registry lookup on the selection path.
- Empty/unavailable/deleted Services do not create new selection events. Already
  captured handles remain valid for in-flight events; historical totals are not
  explicitly cleared on resource updates or deletion.
- Each snapshot owns an `Arc<DegradeSelectionCounter>`, wrapping the real exporter
  Counter. The canonical registry holds only weak references. Final-owner `Drop`
  uses `try_lock` to remove the weak entry without blocking a request. On contention,
  an atomic pending flag schedules dead-weak cleanup at the next registration,
  scrape, or stats call. A pointer check prevents
  an old destructor from removing a concurrently registered replacement.
- Gateway scrapes call `GatewayMetrics::with_live_lb_degrade_counters` around the
  shared synchronous `render_metrics`. Registration, weak-entry removal, and
  keepalive/render use one mutex; only cold operations wait for it. Upgraded strong references outlive
  that lock, so final-owner cleanup cannot recursively lock it.
- Before rendering, `increment(0)` advances the pinned Counter's exporter update
  generation without changing its value. This is required with production idle
  eviction (`MetricKindMask::ALL`, default 600 seconds): a bare cached Counter can
  otherwise be removed from the registry and its later events never exported.
  Only handles with live owners are touched; unrelated series still expire.
- `/api/v1/debug/store-stats` reports `service_discovery.degrade_metric_handles`
  plus bounded Service key samples, including old snapshots still in flight.
  It returns to zero after the final snapshot releases its handle, independently
  of scrape activity and the exporter's remaining historical series.
- `GLOBAL_STORE_TEST_LOCK.lock()` returns a shared cross-module RAII guard that
  restores the global degrade default and complete override map before unlocking,
  including on unwind. It does not promise restoration of unrelated global stores.

Authority: `skills/00-design-rules.md` (observability restraint, hot-path cost,
cross-cutting ownership) and `skills/03-coding/observability/01-metrics.md`.

## Alternatives and limits

[Envoy](https://github.com/envoyproxy/envoy/blob/main/source/extensions/load_balancing_policies/common/load_balancer_impl.cc)
increments `lb_healthy_panic` in its panic selection branch.
[Kong](https://github.com/Kong/kong/blob/master/kong/plugins/prometheus/exporter.lua)
rebuilds its current Target health gauges during metric collection; its Upstream
health threshold rejects traffic rather than implementing this fail-open behavior.
These are supporting comparisons, not project authority or parity guarantees.

An instantaneous Service gauge would need an explicit section aggregation rule
and current-generation collection. That capability is not provided by this Counter.
`rate(...[5m])` describes recent events, not health while idle. No-traffic, disabled,
and healthy Services cannot be distinguished by this Counter alone. Do not divide
by a unique-request counter to claim a selection percentage. Zero registration is
allowed for degrade-capable Services before their first event. Label cardinality
scales with configured Service keys seen by the recorder, not with active requests;
the weak registry contains live owners plus releases deferred since the last cold
operation. Pending cleanup runs before adding registrations, so historical Service
churn does not accumulate even without scrapes.
After the last owner releases, the existing exporter idle policy may expire the
series. A recreated Service reuses any still-retained exporter total; after idle
expiration it starts a new Counter series. Process restart also resets counters.

The keepalive contract depends on `metrics-exporter-prometheus` 0.18.3 using
render-time eviction and `metrics-util` 0.20.4 advancing `Generational` updates
on `increment(0)`. All production Gateway render paths must use the keepalive
boundary; a new raw-render or push-export path must preserve it. This fix does
not certify other cached Gateway metric handles against idle eviction.

The metric follows the threshold actually used by the selector. The broader
captured-EBTP versus current runtime threshold discrepancy is tracked separately in
`tasks/todo/service-degrade-threshold-capture-coherence.md`; this correction does
not certify or change that policy-generation contract.

## Verification and re-evaluation

Targeted tests cover ordinary Service non-registration, publication-time handles
under a different request-time recorder, cross-section events, threshold disable
and re-enable, health/outlier removal, empty membership, deletion, and old-handle
completion after deletion. The global-state test exercises unwind restoration.
Eviction-enabled tests also prove that live handles survive idle scrapes and can
record subsequent selections, an unpinned control series expires, and the degrade
series expires after the last snapshot is released. Concurrent scrape tests cover
event writes and final-owner release while the render lock is held. Recreation
tests prove retained history and weak-entry cleanup without scrapes. A separate
contention test releases the last owner before a scrape can upgrade its weak
reference, proving nonblocking release and subsequent stats-driven cleanup.
Fresh verification on 2026-09-07:

- `cargo test -p edgion-gateway --lib degrade`: 19 passed.
- `cargo test -p edgion-gateway --lib backends::policy::backend_traffic::`:
  50 passed earlier in the same fix, before the final metric cleanup correction.
- `cargo test -p edgion-tests --bin cache_residue retained_degrade_metric_owner`:
  1 passed; retained ownership is detected and released ownership reports clean.
- `cargo fmt --all` and `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets` and
  `cargo clippy --workspace --all-targets`: passed; Clippy retains existing warnings.
- Agent-doc, SSA force, metrics inventory (103 metrics), and Pingora source-policy
  guards: passed. Bare and Kubernetes alert/dashboard artifacts parse and match.
- Independent read-only review: LGTM after idle-eviction and nonblocking-release
  corrections. No full integration suite or `cargo test --all` was run; lifecycle
  coverage here drives real coordinator policy/membership updates in unit tests.

Re-evaluate if selection accounting moves to a different boundary, a new label
is added, current-state monitoring becomes required, or Service snapshot ownership
changes. Do not resurrect the request-written Service gauge.

## Resolution references

- `lb-degrade-mode-metric-lifecycle` — real observability defect; user-approved
  replacement of the gauge by a cached selection Counter, 2026-09-07.
