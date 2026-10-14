---
name: ebtp-atomic-service-snapshot
description: Use when reviewing EdgionBackendTrafficPolicy Service request reads; request-visible fields must come from one captured generation, not live DashMap or store getters.
status: fixed
finding_id: ebtp-atomic-service-snapshot
closed: 2026-09-05
knowledge_type: regression-guards
---

# ebtp-atomic-service-snapshot — one captured Service policy generation — FIXED

## Conclusion

The shared section selector takes an explicit `ServiceDegradePolicy`.
HTTP/gRPC pass `CapturedOverride` from the request's
`EffectiveBackendTrafficPolicy.degrade_threshold_override`; L4 and mirrors pass
`Live`. This closes `service-degrade-threshold-capture-coherence`: the September 7
implementation had read `ServiceRuntimeSnapshot.degrade_threshold` even for
requests retaining an older EBTP generation. That historical implementation
violated this contract; it was not an accepted live-policy exception.

HTTP/gRPC Service requests capture an optional `Arc<EffectiveBackendTrafficPolicy>` at
first `begin_upstream_attempt` and reuse it for later attempts of that request.
Live `get_lb_policy` / `get_authority` / `get_retry_budget` /
`get_circuit_breaker_limit` / `get_connect_timeout` / `resolve_lb_policy` /
`resolve_degrade_threshold` / `resolve_connect_deadline` reads on that proxy path
are **not accepted**. Restoring request-visible DashMaps is **not accepted**.

## Core Rationale

**1. Request-visible fields are one generation**

Load balancer, compiled authority, panic override, retry-budget Arc, circuit-breaker
limit, and connect timeout publish together on the typed snapshot. A Pingora retry
that re-enters `upstream_peer` must not mix those fields from two publications.

The same `EffectivePolicySnapshotData` owns prepared policy resources, the typed
target reverse index, and effective policy Arcs. `partial_update_policies` patches
the old and new target candidate lists; `publish_generation` selects each affected
winner from that index instead of scanning every policy per target. Equivalent
publication retains live handles and skips derived background reconciliation.
The immutable generation is stored before health-check and Service outlier/degrade
side effects are reconciled. Map cloning remains part of publication; this is not
a claim that the entire incremental update is independent of total store size.

ARCH-024 adds a previous-to-final key boundary to partial publication. Add and
Update classifications both allow replacement of a previously published key:
Gateway compression preserves Add for repeated Add and delete-to-Add histories.
EBTP normalizes direct overlapping inputs with remove > update > add, prepares
only the final present value, withdraws previous candidates and reconciles the
previous/final target union once. Invalid replacements withdraw prior eligibility;
equivalent replacements retain raw/effective/live-handle identity and skip
publication and background reconciliation. Health-TLS-invalid winners retain
their separate fail-probe contract; they are not ineligible attachment replacements.

The original defect treated Add as having no previous owner, leaving old candidates
and, on retarget, effective/background state. Ordinary Controller runtime saves
emit Update; this correction concerns supported replacement Add input, not evidence
of a normal deployment incident. The misleading new-only Add comment in the shared
ConfHandler interface is corrected without changing compression or its write lock.
Prepared-value replication and outer-map cloning remain separate costs.

ARCH-023 removes nested prepared-value replication: the resource map and reverse
candidate lists share one immutable `Arc<PreparedPolicy>` per resource. The
complete deduplicated target list is owned once, even for unrestricted Service
fanout. Existing semantic raw-resource retention runs before prepared-owner reuse,
including active health configuration, resolved material/errors and resource
identity. A real resource change replaces the prepared owner while retained
snapshots remain valid. Raw-policy pointer predicates, delta ordering and request
capture remain unchanged by the sharing refactor; ARCH-024 owns the delta
normalization described above. Sharing does not establish measured heap,
throughput or latency gains.

`distinct_service_fanout_shares_prepared_owner_across_publications` exercises 24
distinct Services, unique prepared target-list storage, unrelated partial updates,
equivalent newly allocated full/partial replay, true replacement and retained old
snapshots. The existing AI handle regression directly checks logical circuit-breaker
retention and replacement, and the health winner regression checks prepared-owner
retention on equivalent replay and replacement on health-error repair and resolved CA material change.

**2. Capture is per request, not per TCP keepalive**

A new request on a reused connection loads the current snapshot. A retry of the
same request keeps the already captured Arc. Live counts (`CbPermit`, LeastConn
inflight, `RetryBudget` counters, outlier ejections) stay on those Arcs and are
not copied into the generation.

**3. A missing capture is “no policy”**

Service/gRPC readers must not fall back to the live store when
`BackendContext.effective_policy` is `None`. gRPC capture must not require
`selected_backend`. AI continues to capture at plan time onto `AiHttpState`.

Absence must also survive retries: `begin_upstream_attempt` captures only when
the request's incremented `try_cnt` is one. Testing only `effective_policy.is_some()`
does not establish that capture is complete: a first attempt with no matching
policy leaves `None`, and a later attempt could otherwise adopt a newly added
policy. The 2026-09-05 fix covered existing policy Arcs but missed this absence
case; the 2026-09-07 retry-budget follow-up adds the first-attempt gate and a
`None -> Some` hot-reload regression. Existing requests retain absence, new
requests capture the added policy, and old retries do not fund its budget.

**4. Captured Service overrides and live runtime have different boundaries**

`backends::try_get_peer` passes the captured degrade policy to the shared
ordinary Service/controller-resolved ExternalName selector. Native pseudo-kind
ServiceEndpointSlice dispatch is no longer present.
`CapturedOverride(Some(0))` disables degrade even if a newer Service or global
threshold enables it. `CapturedOverride(None)` uses the current process-global
default, never the live Service override. EGC remains independently updated;
this accepted cross-store default boundary does not merge EGC into EBTP.

Each HTTP/gRPC peer attempt captures one actual Service projection, checked
section and membership-matched runtime snapshot. Actual Service type selects
EndpointSlice or ExternalName DNS; its source/hostname/annotations/port/appProtocol
remain coherent through the attempt, including DNS await. Sticky revalidation
uses that same capture. This per-attempt geometry is separate from the retained
request policy Arc and does not freeze geometry across retries.

Each selector invocation uses that runtime snapshot for membership
and health/outlier eligibility. Neither retries nor this threshold argument freeze
endpoints or their health. L4 and mirror selection retain RoundRobin and resolve
the already published runtime threshold per selection; they do not acquire the
HTTP/gRPC request capture contract.

## Fix Suggestions Not Accepted

- Restore live `get_lb_policy` / `resolve_lb_policy` on the HTTP/gRPC `try_get_peer`
  path — reopens mixed-generation LB versus timeout/authority/retry.
- Keep `resolve_connect_deadline` → `get_connect_timeout` as the Service timeout
  source after capture exists — a half-migration.
- Let HTTP `select_service_endpoint` call `resolve_degrade_threshold` internally — the captured
  override never pins panic for that request.
- Gate Service capture on `selected_backend.is_some()` — gRPC has only
  `service_key` and would keep a live fallback.
- Freeze health/outlier **status**, inflight counts, or endpoint lists into the
  snapshot — those are live runtime, not configuration.
- Swap `RetryBudget` / logical-CB / AI outlier Arcs on equivalent conf_sync
  replay — live counters must not change object identity on a no-op resync.
- Treat L4/`request_mirror` live runtime threshold reads or `request_mirror` Host-as-address
  as this finding — those are documented residuals, not HTTP/gRPC mixed-generation
  defects.

## Re-evaluation Triggers

Re-evaluate against current evidence, including:

- L4 or `request_mirror` grow a `BackendContext` and need the same capture
  contract (separate task; do not silently change L4 LB away from RoundRobin).
- Prepared fields, raw-resource equivalence or candidate ownership change; verify
  shared allocation, health material/error replacement and retained generations.
- A new Service request reader is added that loads policy configuration after
  `begin_upstream_attempt` without using the captured Arc.
- Attempt counting or capture initialization changes, including the case where
  the first lookup returns no policy.

## Reference Cases

- Historical implementation: `f5e4781e`; captured degrade-threshold correction:
  `b179660c`. This fixed rule is the durable record after the implementation
  task was archived.
- Source task ID: `backend-traffic-policy-atomic-service-snapshot` (already
  implemented; task cleanup authorized on 2026-09-10, without a new runtime fix).
- `edgion-gateway/src/backends/policy/backend_traffic/store.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`
- Retry-budget window rotation is covered by
  [retry-budget epoch coherence](retry-budget-epoch-coherence.md).
- Live health/outlier state is intentionally separate from the policy generation
  and uses the [Service runtime selection snapshot](../../lb/regression-guards/service-runtime-snapshot-selection.md).
  Degrade metric lifecycle is covered by
  [selection Counter ownership](../../lb/regression-guards/degrade-selection-counter-ownership.md).

## Resolution references

- `ebtp-atomic-service-snapshot` — 2026-09-05.
- `service-degrade-threshold-capture-coherence` — real correctness defect;
  restored the captured Service override at the shared selection boundary,
  verified 2026-09-08.

## Verification

ARCH-024 regression coverage uses the actual EBTP ConfHandler through CacheData's
synchronous flush seam in `compressed_replacement_add_reconciles_backend_traffic_policy_ownership`.
It covers delete-to-Add retarget, same-target changed Add, equivalent replay,
invalid replacement, retained old snapshot and HC/outlier/degrade lifecycle, with
sequential full/partial final-state comparison. Store regressions cover direct
overlap, sibling promotion and typed AI replacement/handle retention. The new
CacheData regression failed against the original implementation at the stale
old-target candidate assertion before the fix. Fresh post-fix results are recorded
in the implementation task; historical results below do not validate this change.

Task cleanup verification on 2026-09-10: independent read-only review confirmed
the original defect is already fixed and the selected task materials contain no
unique unfinished scope. `cargo fmt --all`, workspace check and Clippy, agent-doc
validation, SSA-force, metrics inventory, and Pingora source-policy guards passed.
Clippy emitted existing warnings in unchanged Rust files. This closure changes
only task records and knowledge references; runtime tests were inspected but not
rerun. The runtime results below remain historical verification from September 8.

The correction adds `service_degrade_capture_controls_actual_selection_across_retries`
and `captured_degrade_threshold_uses_live_health_and_a_cold_counter_at_zero_live_threshold`.
They exercise actual endpoint selection and Counter outcomes, threshold increases
and decreases, explicit disable, absent policy/override, policy removal, retry
capture, global-default updates, and live health transitions. Runtime health is
controlled directly without network probes; these tests do not certify probe IO.
The `ServiceDegradeThreshold` dynamic scenario verifies new-request hot updates
with shared-stack traffic and actual degrade Counter deltas. It does not claim
an in-flight network retry barrier; deterministic capture coverage owns that case.

Historical verification on 2026-09-08 (not fresh R2c gates):

- `cargo test -p edgion-gateway --lib degrade`: 21 passed.
- The resulting Gateway libtest executable also ran the complete selector,
  discovery coordinator, and `pg_upstream_peer` test modules: 10, 27, and 88
  passed respectively. These groups overlap the degrade-filter run.
- `cargo build -p edgion-gateway -p edgion-tests --bin edgion-gateway --bin test_client`:
  passed; `test_client --list-dynamic-scenarios` includes `ServiceDegradeThreshold`.
- `test_client -g --dynamic-scenario ServiceDegradeThreshold`: passed on the
  shared Gateway/Controller/test-server topology. Observed disabled -> degraded ->
  disabled traffic and verified actual Counter deltas, then removed scenario resources.
  Startup and tests ran in one execution session to retain the child processes.
- `test_client -g -r HTTPRoute -i Backend/DegradeThreshold`: 1 passed.
- `cargo fmt --all`, format check, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed. Clippy reported existing
  warnings outside the new code; no warning cleanup was included.
- Agent-doc consistency, SSA-force, metrics inventory, Pingora source-policy,
  and diff-whitespace guards: passed.
- Independent read-only review: LGTM. No full integration suite or
  `cargo test --all` was run.

## Complete precedence across transport (ARCH-055)

The former seconds-based Gateway key and whole-resource status seeding could
reverse two same-second fractional candidates: `b` at `.100` must beat `a` at
`.900`. Resources now owns complete-instant ordering with explicit missing-last
semantics. Both Controller handlers overwrite a schema-hidden serialized
`preparedPrecedence` carrier from source metadata, and policy status seeding
reuses typed status replacement to preserve that metadata. Gateway cold
eligibility requires preparation, and explicit resource equality includes it;
there is no rounded-metadata fallback. Prepared missing time is distinct from
unprepared input. Existing snapshot/target ownership and native TLS preparation
remain authoritative; this does not supersede ARCH-023/024/099.

Regression coverage uses one shared checked wire fixture. Controller tests run
canonical static parsing, real registered policy processors, serial sibling
queues, persisted status seeding and actual ServerCache List/Watch serialization.
Gateway dispatch tests consume the same complete expected wire objects through
production List/Watch decoders and real policy ConfHandlers. Fraction-only updates,
missing preparation, captured snapshots and equivalent replay have store-level
regressions. Resources and Controller ingress tests cover chrono extrema,
fractions, extended years and leap seconds. This composes in-process boundaries;
it does not claim socket-level integration or a Kubernetes production incident.
Fresh execution results belong to the implementation verification record.

A new Gateway rejects unprepared output from an old Controller until coordinated
upgrade/reprocessing supplies current prepared policies. Re-evaluate if source
metadata ingestion, status seeding, timestamp serialization, eligibility,
resource equality or precedence ownership changes.
