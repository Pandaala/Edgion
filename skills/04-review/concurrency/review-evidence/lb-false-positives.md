---
name: lb-false-positives
description: Use when reviewing load-balancer module concurrency findings; distinguishes real RwLock+ArcSwap consistency gaps from false-positive TOCTOU reports on single-structure ArcSwap usage.
knowledge_type: review-evidence
---

# Common LB Module False-Positive Scenarios

> Source: 2026-04 LB module review; conclusions verified one-by-one against source code.

---

## Cross-Structure RwLock+ArcSwap Consistency Gap IS a Real Issue

**Distinction from the ArcSwap single-structure TOCTOU false positives below**: when `policies` (RwLock) and `reverse_index` (ArcSwap) are two *separate* fields that must stay in sync, releasing the write lock on one before storing to the other creates a real inconsistency window — not a benign TOCTOU.

**Fixed pattern** (g7-005, `BackendTLSPolicyStore::update`): `self.reverse_index.store(Arc::new(new_index))` was called *after* the `policies` write lock was released. Fixed by moving the store call inside the write lock scope, so both fields update atomically from any reader's perspective.

**Rule**: If a reviewer finds `rwlock.write()` scope drops before `arc_swap.store()` where the two structures are semantically linked (e.g., a forward map and its reverse index), treat it as a real confirmed bug. The `ArcSwap::store` is lock-free and safe to call inside any RwLock write scope.

**Historical contrast with single-structure ArcSwap TOCTOU**: the old `BackendSelector::init` discussion below concerns a removed lifecycle. It is not current guidance for shared-state initialization. Cross-structure races remain different: readers can observe the new state of one field but the old state of the other.

---

## Historical BackendSelector initialization evidence

> Inactive implementation premise. ARCH-016 removes BackendSelector's `init`,
> `is_initialized` and inner ArcSwapOption. Route-generation builders compile the
> selection plan before publication; requests only select from its private state.
> See [BackendRef selector generation ownership](../../lb/regression-guards/backend-ref-selector-generation-ownership.md).

The April 2026 review considered a lazy `is_initialized`/`init` check-then-store
race. Its evidence was that concurrent initializers derived the same immutable
route inputs, replacement created a fresh route generation, and ArcSwap prevented
tearing or use-after-free. That historical review rejected the reported race;
it did not require the initialization API or approve future in-place writes.

Before ARCH-016, current HTTP/gRPC/L4 builders already initialized on the cold path,
so the original request-triggered race premise no longer matched the call chain.
The refactor removes that unused lifecycle as architecture debt, without claiming
a newly observed race. WRR's atomic cursor remains legitimate request-time state
shared by matches and hostname groups within an owning rule generation.

Re-evaluate any future proposal for in-place selector replacement against its
actual publication owner, weight/error contracts and cursor lifetimes; the old
same-input reasoning is not approval for a new mutable configuration path.

---

## ConsistentHashRing Not Supporting Unix Domain Socket is Not a Real Issue

**False-positive scenario**: In `ConsistentHashRing::build` and `sp_affinity_build`, `filter_map` only handles `SocketAddr::Inet`; `SocketAddr::Unix` is silently dropped and UDS backends do not participate in consistent hashing or session persistence.

**Reality**:

1. **Discovery layer only produces Inet addresses**: Backend addresses come from K8s Endpoint/EndpointSlice IP:port, constructed via `"ip:port".parse::<SocketAddr>()` (`endpoint_slice/discovery_impl.rs`, `endpoint/discovery_impl.rs`); always `Inet`.
2. **UDS backends cannot appear in the current architecture**: No discovery/construction path generates a `SocketAddr::Unix` `Backend`. The `else` branch of `filter_map` is exhaustive matching for the pingora type system; effectively dead code.
3. **SP affinity is the same**: Inet-only filtering in `sp_affinity_build` / `sp_affinity_get_or_build` does not affect actual behavior.

**Verdict**: An uncovered branch at the type-system level but unreachable at runtime. If UDS backend discovery is introduced in the future (e.g., sidecar scenarios), the CH and SP paths must be extended in sync; that will be handled naturally then. No fix is needed now, nor a warn log (which would create meaningless noise).

---

## EWMA Integer Arithmetic Precision Loss

> **Superseded 2026-09-04.** `update_ewma`, the global alpha and the 1 ms seed were all deleted
> with the fixed-alpha estimator; see
> [../../12-algorithms/lb-peak-ewma.md](../../../12-algorithms/lb-peak-ewma.md). The verdict below
> is retained for re-reports against older revisions. Precision loss in the replacement is
> bounded by invariants 3-6 of that entry, not by the reasoning here. is Not a Real Issue

**False-positive scenario** (against pre-2026-09-04 code; `update_ewma`, the global alpha and the 1 ms seed were all deleted with the fixed-alpha estimator — see [../../12-algorithms/lb-peak-ewma.md](../../../12-algorithms/lb-peak-ewma.md)): `update_ewma` uses percent-based integer alpha (0-100) and `/ 100` truncating division to update `AtomicU64`, discarding remainders at every step. Initial value 1ms is detached from real RTT. Global alpha cannot be configured per-service.

**Reality**:

1. **Truncation error is negligible**: Numerical simulation shows the cumulative difference between integer and float remains within ~2µs. EWMA is used for comparing backend latency differences which are typically at the ms level; truncation error is only ~0.05% of the signal and has no impact on routing decisions.
2. **Initial value converges quickly**: From 1ms initial to 50ms real RTT, with alpha=20 it converges to 90%+ in about 10 observations. Sending a few extra requests to a new backend is essentially reasonable probing behavior.
3. **alpha granularity is sufficient**: EWMA itself is a heuristic approximation; 1% step has no real impact on load-balancing decisions.
4. **Global alpha is a design choice**: Per-service alpha is a feature enhancement, not a bug.

**Verdict**: The implementation is correct; precision is fully sufficient for load-balancing scenarios; no fix needed.

---

## Session Persistence CRC32 Collision is Not a Real Issue

**False-positive scenario**: SP affinity map uses CRC32 hash of `ip:port` as token key; in 2^32 space there is birthday-paradox collision risk; `HashMap::insert` silently overwrites and loses backends. Also concerns about token security, cache consistency, hard-coded 1024 cap, etc.

**Reality**:

1. **Collision probability is extremely low**: A typical K8s Service has 3-50 endpoints; at n=50 the collision probability is 2.85e-7 (about 1 in 3.5 million Services). Even at the 1024 cap it is only 0.012%. Empirically zero collisions with 1024 structured K8s IPs.
2. **Token security is sufficient**: CRC32 is non-reversible; the cookie already has HttpOnly + SameSite=Lax. Guessing a token only sticks the user to a backend, not a privilege escalation. Standard gateway practice.
3. **Cache consistency is a shared pattern**: SP_AFFINITY shares `invalidate_selector_cache` with RR_CACHE and CH_CACHE, triggered by the endpoint discovery handler; not an SP-specific issue.
4. **1024 cap is reasonable**: K8s Services rarely exceed this; nice-to-have at most.
5. **`sp_affinity_build` is dead code**: Confirmed no callers; P3 code hygiene, doesn't affect correctness.

**Verdict**: The design is reasonable; collision risk is negligible at real-world scale; no fix needed.

---

## Historical: Removed Route-Owned PolicyStore Cleanup

> **Superseded.** HTTPRoute no longer owns or synchronizes load-balancing
> policies. The former `PolicyStore`, `remove_by_route`, and
> `batch_remove_routes` path was removed. Typed `EdgionBackendTrafficPolicy`
> now owns backend traffic and load-balancing policy through
> `BackendTrafficPolicyStore`. The text below is retained only as evidence for
> reviews of older revisions and must not be applied to current code.

**False-positive scenario**: `remove_by_route` calls `clone()` on each `ServiceLbPolicy` inside the `DashMap::retain` callback and writes back, introducing deep-copy overhead proportional to store size. `batch_remove_routes` performs full table scans repeatedly per route.

**Reality**:

1. **Control-plane cold path**: `remove_by_route` is only called when an HTTPRoute is deleted/synced; not a per-request data-plane path; frequency is extremely low.
2. **Data scale is tiny**: DashMap size = number of Services with LB policies configured (typically single digits to a few dozen). `ServiceLbPolicy` only contains a short `Vec<LbPolicy>` + small `HashSet<String>`; clone overhead is negligible.
3. **`changed` semantics are correct**: Detecting only policies length change is reasonable — `route_refs` changes do not change actual LB behavior, no need to trigger an endpoint slice update.

**Verdict**: The code could be simpler (in-place modification rather than clone+write-back), but performance impact is zero on a control-plane cold path with tiny data scale; no fix needed.

---

## BackendCleaner Graceful-Shutdown Finding Is Historical

**Historical scenario**: an older `BackendCleaner::stop()` only set a flag without waiting for the
task to exit, while `start()` discarded the `JoinHandle`. At that time there was no production
`stop()` caller, so the cleaner was treated as process-lifetime state and the proposed independent
shutdown protocol had no operational value.

That evidence must not be applied to current code. The lifecycle has since changed:

1. `BackendCleaner` retains its task in `handle: Option<JoinHandle<()>>`.
2. `stop()` cancels the token, takes and awaits the handle, then installs a fresh token so a later
   `start()` can run normally.
3. Gateway auxiliary-service startup retains the cleaner in its shutdown task and calls
   `cleaner.stop().await` after the production shutdown signal fires.
4. Unit tests cover clean start/stop, duplicate-start protection, and restart after stop.

**Current verdict**: the old "no handle / no caller / no explicit shutdown" claim is superseded by
the implemented lifecycle. Re-review current cancellation, await, reset, and production shutdown
wiring directly; do not close a new finding using the historical process-lifetime rationale.

---

## select_by_min_metric BinaryHeap Small-Scale Performance is Not an Issue

**False-positive scenario**: `select_by_min_metric` creates a BinaryHeap on every selection; for a small pool of 3-10 backends, heapify overhead exceeds linear scanning.

**Reality**: For each backend, `is_backend_active` (DashMap lookup) + `metric_fn` (DashMap lookup) is required — two concurrent map lookups are the real time cost. For 3-10 elements, the difference between heapify and linear scan is in nanoseconds, completely dwarfed by DashMap reads. `with_capacity(n)` for n<10 allocates a few dozen bytes — modern allocators treat this as near no-op.

**Verdict**: The bottleneck is DashMap lookup rather than data structure choice; introducing a threshold branch for micro-optimization adds complexity instead; no fix needed.

---

## Historical try_get_peer Duplicate Mode Arms

**Original issue**: The Service/ServiceEndpointSlice match arms of `try_get_peer` had ~25 lines of duplicated upstream-fill + peer-build logic.

**Historical fix**: extracted `build_peer_from_lb_selection` to share upstream metadata and peer construction. The native pseudo-mode arms have since been removed. Current HTTP/gRPC dispatch derives actual Service type from `CapturedServiceSelection`, with one shared section selector for ordinary/controller-resolved Services and the canonical DNS peer stage for self-resolved ExternalName. The historical duplicate-arm count does not establish current ownership, hot-path cost or correctness; inspect the actual captured source and both peer stages.

---

## Historical: Removed ServiceLbPolicy Route-Reference Union

> **Superseded.** The route-reference union and `ServiceLbPolicy::add_route_ref`
> belonged to the removed route-owned LB policy mechanism. Current policy
> arbitration is performed by `BackendTrafficPolicyStore` from typed
> `EdgionBackendTrafficPolicy` resources. The text below is historical only.

**False-positive scenario**: `policy_store.rs`'s `add_route_ref` (formerly `merge`) does "skip if exists, else push" on new policies; inconsistent with "last-write-wins"; when multiple routes reference the same service, old policies are not overwritten.

**Reality**:

1. **Union semantics is the product definition**: A Service can be referenced by multiple HTTPRoutes, and the LB policy declared by each route should be respected by that Service; the policy list is the union of all referencing routes, selected per-request by the LB layer rather than arbitrated at the store layer.
2. **Lifecycle is managed by route_refs**: A policy is only cleaned up when the last referencing route is removed via `remove_by_route`; there is no scenario of "a route unilaterally clearing its previously registered policy", nor is such semantics needed.
3. **Already commented in 2026-04 P2-03**: Comments in `policies/lb_policy/policy_store.rs` explicitly write out the difference between union semantics and LWW.

**Verdict**: Union + reference-counted cleanup is intentional design; future reviews should not file this as "not last-write-wins".

---

## ConfigClient Multiple ClientCaches with No Cross-Cache Transaction is Not an Issue

**False-positive scenario**: `conf_sync/conf_client/config_client.rs` holds a dozen-or-so `ClientCache<T>` simultaneously; updates across caches lack a cross-cache transaction/consistency barrier; some resource dependencies (e.g., EndpointSlice depends on Service, HTTPRoute depends on Listener resolved) are not explicitly ordered or documented in code.

**Reality**:

1. **Configuration sync is eventually consistent by nature**: Controller→Gateway uses gRPC list/watch; the server side is already version-driven for events; the client cache may at any moment be slightly ahead/behind other caches; all reader paths "downgrade if not found" or "wait for next rebuild".
2. **Readiness has a barrier already**: `required_cache_names` + `is_ready()` blocks business traffic during startup until critical caches are all in place (see `config_client.rs:40-42`); short-term inconsistency during runtime incremental updates is tolerated by readers.
3. **Joint request invariants are atomic inside submodules**: HTTP/GRPC ConfHandlers retain the family rebuild mutex while committing the exact compiled tables and admission facts to PluginPublisher, then release it before unrelated contribution updates. Early RequestId and later route matching use one captured listener publication. The former independent route-table/admission stores had a real 500 race; cross-cache eventual consistency does not excuse that mismatch. Deterministic tests retain old requests across actual full/partial commits, removal and recreation, preserving exact route-Arc admission checks.
4. **Strong transactions cost too much**: Introducing cross-cache write locks/version barriers in the data plane would serialize every config sync; not worth it.

**Verdict**: Cross-cache eventual consistency + strong ordering inside submodules + readiness barrier is a tradeoff design; no transaction or global ordering documentation needs to be added at the ConfigClient layer.

---

## pick Multi-Parameter Does Not Need Refactoring

**False-positive scenario**: `pick` (in `selector.rs`) wraps `EdgionErrorCode` values (per-policy combination); a suggestion may be to further abstract into source enum mapping.

**Reality**: The only caller is `select_from_endpoint_slice` (defined in the same file nearby). Error code variants are only used in `backends/mod.rs` repo-wide; no external consumer. New LB algorithms are added very infrequently. Refactoring to a source enum + mapping function would only convert explicit parameters to implicit mapping, adding a layer to debugging.

**Verdict**: Single caller, same-file visibility, very low extension frequency; no refactoring needed.
