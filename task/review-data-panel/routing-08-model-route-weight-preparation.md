# AI model-route selector and weight preparation

Status: deferred optimization/design decision for user review. No runtime change.
Source baseline: `36b1565a1`.

## Evidence

The request callback reaches `AiProxy::run_request_with_store_and_entropy`, which
captures `route_unit.backends` and one AI runtime snapshot before calling
`ModelRouteTable::decide`. Model listing also calls `decide` for proposed model
IDs. In `ai/routing/model_router.rs`, every successful decision creates a default
weight Vec from backend declarations. For a matched rule with candidates, each
selector scans the backend slice for exactly one matching identity and the
results populate a temporary HashMap keyed by raw declaration slot.

Identity comparison uses the canonical group, kind, namespace, name, section and
occurrence fields. A missing or ambiguous selector returns InvalidSelector.
Candidate entries are resolved in order before weights are applied. For permitted
AI slots, an explicit nonnegative weight overrides, absent weight inherits the
schema default, and an unselected slot becomes zero. A negative override returns
InvalidWeight when applied to a permitted slot. Nonpermitted slots retain their
original declaration weights. Direct duplicate selector entries currently leave
the last override for a slot; canonical config validation rejects duplicates.

The caller additionally applies decision weights only when the captured runtime
contains the backend. A missing runtime remains an invalid declared share rather
than being normalized away by a model filter. Live availability, permissions and
invalid-share admission are therefore not replaceable by a cached list of healthy
or selected endpoints.

`AiProxy::from_config` compiles configuration without a backend-membership input;
its ModelRouteTable is not intrinsically paired with a particular route's
`Arc<[CompiledBackend]>`. Resources' `model_route_context_invalid_reason` also
resolves selectors against canonical declaration identities during admission,
but that validation is not an owned, generation-bound runtime weight projection.
It does not by itself justify removing the Gateway decision checks.

## Decision alternatives

- Retain the current bounded calculation if its cost is insignificant. Validated
  model routes have at most 16 candidates; measure realistic request/listing
  workloads before choosing a more complex owner.
- Evaluate bounded temporary override storage instead of HashMap, preserving
  resolution/error order and duplicate behavior. Compare linear lookup costs,
  stack/heap size and direct-construction inputs that bypass validated limits.
- Prepare immutable structural selector/weight results for an exact route and
  plugin-configuration pair. Existing publication is a candidate ownership
  location, not an established ready pairing interface. Design generation,
  authorization and invalidation rules first; do not add a request-time global
  cache or pin a reusable plugin to whichever route first invokes it.

The latter alternative may replace repeated selector scans and temporary storage,
but must account for per-route/per-rule retained weights, old readers, rebuilds
and original schema ownership. Do not precompute live runtime presence, health,
credential readiness, quota, request header matches or model visibility.
No implementation, representation or new publication seam is selected here.

## Acceptance

Preserve raw declaration slots, occurrence identities, default/zero/negative
weights, denied and missing shares, exact-one selector resolution, direct-input
failure order and the production validation boundary. Preserve model/header
matching, specificity, canonical rewrite restrictions and rewrite-before-quota
identity. Keep listing visibility and ordinary request decisions consistent.

Verify one plugin used with different backend lists, route reorder/replacement,
plugin updates, reference-permission changes, missing/restored runtime objects,
publication skew, retained old generations and mismatched prepared owners. Any
prepared result must prove its exact input pair or take the reviewed failure
path; resource identity or a name-only cache key is insufficient. Do not infer
permission from lookup identity. Keep admission, deadline and cancellation
ownership unchanged.

Measure matched/unmatched decisions, early-invalid selectors and model listing
across backend/candidate counts. Include preparation time, allocations, retained
memory, updates and concurrent old readers, with baseline and profile stated.
A source-level repeated scan alone is not a measured throughput regression.
Use independent IMPL plan/final review and freeze an implementation allowlist
before changing code. Add no tracing, metrics, schema knobs or parallel cache.

## Original knowledge and verification

- [Precompiled AI backend ownership](../../skills/04-review/ai-routing/regression-guards/precompiled-backend-lookup-keys.md)
- [Logical-model quota identity](../../skills/04-review/ai-routing/regression-guards/identity-model-quota-uses-requested-logical-name.md)
- [Prepared attempt admission ownership](../../skills/04-review/ai-routing/regression-guards/prepared-attempt-owns-admission-input.md)

Fresh unchanged-source runs passed four model-router and 21 AI HTTP proxy tests
(25 total). Existing tests cover selector occurrence, invalid shares, missing
selectors, weights, model visibility and canonical request behavior; they do not
prove a proposed generation-pair cache or prepared weight representation correct.
No prototype, benchmark, live integration or full suite ran for this audit.
