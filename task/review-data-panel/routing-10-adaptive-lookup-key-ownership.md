# Adaptive telemetry lookup-key ownership

Status: deferred optimization/API decision for user review. No runtime change.
Source baseline: `09292834e`.

## Evidence

`ai/http/proxy.rs::capture_adaptive_states` runs for LowestLatency and LowestUsage
selection. After resolving the logical model against a retained backend and
checking candidate.model, it constructs a temporary PolicyTargetKey and
AdaptiveTelemetryKey for lookup. Namespace, backend name, terminal route UID,
rule identity and logical model each become owned Strings. These five String
constructions copy nonempty content; they are not universally five allocations
because empty strings need not allocate. The key is discarded after lookup.

`ai/routing/adaptive.rs::AdaptiveTelemetryStore::get` reads an immutable
ArcSwap<HashMap<AdaptiveTelemetryKey, Arc<AdaptiveTelemetryState>>> with a short
guard and clones only the returned state Arc. It neither creates state nor
mutates the map. Each candidate currently performs its own snapshot load.
Initial planning and quota downgrade both call capture_adaptive_states; the
previous store-accessor optimization removed an outer Arc acquisition, not these
owned lookup-key copies.

Key construction rejects empty route UID, rule identity or logical model and a
non-EdgionBackend target kind. It does not validate namespace/name. Equality and
hashing include all typed fields; the UID distinguishes route recreation.
Cold contribution recomputation enumerates backend.models.keys(), while runtime
resolve_model also accepts wildcard-prefix matches. Therefore a catalogued
runtime model need not have an exact published adaptive key. Misses are valid;
normalizing concrete names into wildcard patterns or creating state on a miss
would change policy and potentially admit unbounded request-derived state.

## Decision alternatives

- Keep temporary owned keys if bounded request cost does not justify another
  lookup interface or data representation.
- Add canonical borrowed structured lookup to the existing store. Prove equality
  and hashing coherence for every component, including target kind and namespace,
  with the owned key. Compare dispatch/indirection and hash costs as well as copies.
  Do not concatenate components with an ambiguous delimiter, scan the entire map
  as an assumed improvement, or rely on an unchecked raw hash.
- Prepare an immutable identity owner at an appropriate route/plugin/backend
  publication boundary. This requires an explicit design for exact generations,
  logical-model selection, old readers, invalidation and unknown-model misses;
  no ready pairing interface is presumed. It must not introduce a second cache.

No alternative is selected. Preserve per-candidate snapshot observation unless a
separate reviewed design explicitly changes it; loading one snapshot for all
candidates is not merely a borrowed-key substitution. Do not retain a map guard
across async work. Returned state ownership must still survive map retirement.

## Acceptance

Verify owned/borrowed lookup parity for hits, misses, all differing key components,
empty rejected components and current empty target-component behavior. Include
route delete/recreate, rule/target changes, old state Arcs after retirement,
concurrent cold reconciliation, missing backend generations and quota downgrade.
Preserve exact-key behavior for wildcard-resolved names, request lookup's lack of
map mutation, and the existing count/key/residue interfaces.

Keep adaptive counters, ticker publication, selection sequence, request deadlines,
admission ordering, quotas and logical/provider model identities unchanged. No
new logging, metrics, schema knobs, lazy state creation or global interner.
Measure representative candidate/model counts, lookup hit/miss mix, allocation
and copied bytes, CPU, publication cost and retained memory before choosing a
representation. Source evidence alone establishes no throughput improvement.
Freeze implementation scope and obtain independent IMPL plan/final review.

## Original knowledge and verification

- [Compiled AI ownership](../../skills/04-review/ai-routing/regression-guards/precompiled-backend-lookup-keys.md)
- [Prepared attempt ownership](../../skills/04-review/ai-routing/regression-guards/prepared-attempt-owns-admission-input.md)
- [Store count/accessor scope](../../skills/04-review/ai-routing/regression-guards/ai-store-single-count-accessors.md)
- [Prior borrowed store accessor](INDEX.md#borrowed-adaptive-telemetry-store-accessor)

Fresh unchanged-source verification passed 22 adaptive-routing and 22 AI proxy
tests (44 total). These cover existing lookup/publication and request behavior,
not a new heterogeneous-key API or prepared identity representation. No prototype,
benchmark, full suite or live integration ran for this audit.
