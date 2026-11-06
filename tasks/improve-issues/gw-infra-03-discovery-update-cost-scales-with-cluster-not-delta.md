# gw-infra-03: Service-discovery update cost scales with total cluster size per event (quadratic on relist)

- **Category**: design-error
- **Severity**: P2 (worth doing) — downgraded from P1 by independent verification
- **Scope**: large-direction
- **Location**: `edgion-gateway/src/backends/discovery/coordinator.rs:911-956` (`replace_endpoint_slices`, `update_endpoint_slices`), `:958-1068` (`transact`), `:1169-1180` (`clone_published_map`), `:1385-1415` (`build_service_projection`), `:1488-1515` (`projection_epoch_requirement`), `:1372-1383` (`endpoint_slice_matches_service`), `:2013-2056` (`generation_stats`), `:1837-1902` (`materialize_section`); `lb/compiled_selector.rs:273-302` (`CompiledSelectors::build`); `lb/selection/consistent_hash.rs:21-71`

## Summary
All Service/EndpointSlice mutations are serialized through one `Mutex<CoordinatorState>`. Each transaction
does work proportional to the number of Services and EndpointSlices in the whole cluster rather than to the
size of the change. For each affected Service it linearly scans every EndpointSlice twice, and each changed
section then eagerly compiles all four LB selectors, including a ketama ring with 160 points per endpoint.
Large clusters with high churn are therefore expensive: rolling deploys, HPA, readiness flaps and full relists
after a controller reconnect. A relist is O(Services x EndpointSlices).

## Current behavior (evidence)
Per `transact` call:
- `let mut staged = state.clone();` deep-clones both source maps (`HashMap<String, LiveSourceRecord<_>>`,
  every key `String` plus record), and `let mut preflight_state = staged.clone();` clones them again
  (`:960`, `:976`).
- `clone_published_map(&current.services)` re-allocates every Service key of the published generation (`:967`).
- For every affected key, `projection_epoch_requirement` (`:1488`) and then `build_service_projection`
  (`:1385`) each run
  `state.endpoint_slices.iter().filter(|(_, s)| endpoint_slice_matches_service(&s.resource, key))`,
  which is a full scan of *all* slices with a label lookup and string compares per slice. Both run
  `normalize_sections` and `service_content_fingerprint` (SHA-256 over the Service's slices) for the same
  Service.
- `generation_stats` iterates all source records and all published Services (`:2013`).
- `replace_endpoint_slices` (full relist on conf-sync resync) marks *every* old and new owner Service as
  affected (`:913-919`). With S Services and E slices, this is about 2·S·E slice-match evaluations, plus S
  fingerprint computations, while holding the coordinator lock. The content-fingerprint short-circuit avoids
  re-materialization but not the scans.
- When a Service's fingerprint changes, `materialize_section` calls `CompiledSelectors::build`
  (`compiled_selector.rs:283`). That eagerly builds RoundRobin, ConsistentHash (a `pingora_ketama::Continuum`
  with 160 x N points, hashed and sorted), RR fallback, LeastConn and EWMA selectors, plus two
  `HashMap<SocketAddr, Arc<LbBackend>>`. This happens whether or not any route uses consistent hashing. Each
  section is rebuilt in full, so one pod change in a 1 000-endpoint Service hashes and sorts about 160 000 ring
  points.
- Per-change downstream work also scales with Service size because every endpoint is re-identified and
  re-activated (see gw-infra-02).

Rough order of magnitude for 5 000 Services and 10 000 slices: each single-slice update performs about 20 000
slice-match checks, two full map clones of 15 000 entries and one 5 000-entry published-map clone. A relist
performs about 10^8 slice-match checks under the lock, and every conf-sync event for Services or EndpointSlices
queues behind it.

## Industry comparison
- Envoy EDS updates are per cluster (`ClusterLoadAssignment`). A cluster's host set is diffed incrementally
  (`updateDynamicHostList`) and per-priority LB structures are rebuilt only for that cluster. Ring hash and
  Maglev tables are built only for clusters configured with that policy (`lb_policy` is per cluster). With
  delta xDS, unchanged clusters are never touched.
- Kubernetes' own EndpointSlice consumers (kube-proxy `EndpointSliceCache`) keep a
  `service -> slices` index and recompute only the touched Service.

## Impact
CPU spikes and serialized lag in endpoint propagation exactly during deploys and failovers. On controller
reconnect or relist, the gateway can stall discovery updates for a long time. That delays removal of dead
endpoints, which is a correctness problem, not only a cost. Memory churn rises from repeated map clones and
ring rebuilds.

## Suggested direction
1. Maintain a derived `service_key -> BTreeSet<slice_key>` index inside `CoordinatorState`, updated in the
   same staged mutation as `endpoint_slices` and therefore not a separately mutable reverse index.
   `build_service_projection` / `projection_epoch_requirement` then read only that Service's slices.
2. Compute the normalized geometry and fingerprint once per affected Service and reuse them in both the epoch
   preflight and the build, instead of normalizing twice.
3. Replace whole-state clones with copy-on-write maps such as `im::HashMap` / `rpds`, or with
   staged-diff-only transactions, so a transaction allocates O(changed) entries. Do the same for the
   published generation map (a persistent map or `Arc<HashMap>` per shard).
4. Compile selectors lazily per policy (`OnceLock` per `PolicyKind` inside the section), or eagerly only
   for policies referenced by routes targeting that Service. The ketama ring is the dominant cost and is
   unused by most Services.
5. Make `generation_stats` incremental (adjust counters by the delta of affected keys).

## Notes
`skills/04-review/backends/regression-guards/endpoint-slice-ownership-transitions.md` says "Do not restore a
separately mutable Service-to-EndpointSlice reverse index". The proposal in (1) is a *derived* index that is
updated atomically inside the same staged `CoordinatorState` mutation and recomputed from the old and new
owner labels. It therefore preserves that page's invariant, which is ownership derived from the resulting source state
and the union of old and new owners. `skills/04-review/lb/superseded/build-lock-single-mutex-intentional.md`
and `lb/regression-guards/service-runtime-snapshot-selection.md` cover selector publication semantics, not
build cost. `cpu-memory/intentional-design/backend-generation-arc-slice-handle-layout.md` covers handle layout
only. Not tracked in `tasks/`.

## Verification (independent)

- **Verdict**: DOWNGRADED — P1 → P2. The mechanics are confirmed, but the cost is an unmeasured order-of-magnitude estimate. It matters only for very large synced inventories, mainly on full relist. A single-slice update costs tens of microseconds of string compares at the cited scale. This is a scalability improvement, not a correctness defect.
- `coordinator.rs:958-976` `transact` clones `CoordinatorState` twice (`staged`, `preflight_state`) and clones the published map (`:967`) under `self.state.lock()`.
- `build_service_projection` (`:1385-1415`) filters all `state.endpoint_slices` with `endpoint_slice_matches_service` for every affected Service. `replace_endpoint_slices` (`:911-925`) marks every old and new owner as affected.
- `compiled_selector.rs:283-291` `CompiledSelectors::build` eagerly compiles RoundRobin, ConsistentHash (ketama), LeastConn and EWMA for every materialized section.
- No `skills/04-review` page accepts eager selector build cost or full-scan projection cost.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `backends/discovery/coordinator.rs`, `lb/compiled_selector.rs` and `lb/selection/consistent_hash.rs` are unchanged between feature-08-30 and HEAD.
- `transact` (`coordinator.rs:958`) still clones state twice under the lock; `build_service_projection` (`:1385`) and `projection_epoch_requirement` (`:1488`) still full-scan `endpoint_slices` via `endpoint_slice_matches_service` (`:1372`); `CompiledSelectors::build` (`compiled_selector.rs:284`) still eagerly builds every selector including ketama.
