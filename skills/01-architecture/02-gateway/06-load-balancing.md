---
name: gateway-load-balancing
description: Load balancing strategies — RoundRobin, EWMA, LeastConn, ConsistentHash, WeightedSelector, and the backend selection flow; algorithm source is EdgionBackendTrafficPolicy.
---

# Load balancing

> The load balancing module is responsible for selecting a target instance from the backend list.
> The default Service endpoint strategy is `RoundRobinSelector`; a non-default algorithm is configured
> **per-destination** via the `EdgionBackendTrafficPolicy` CRD attached to the Service (not per-route).
> Full external schema: [02-features/03-resources/20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md).

## Load balancing strategies

| Strategy | CRD `loadBalancer.type` | Selector-side `PolicyKind` | Description |
|----------|--------------------------|------------------------------|-------------|
| RoundRobin | `RoundRobin` (default; section may be omitted entirely) | `PolicyKind::RoundRobin` | Weighted index-array round robin via `RrCore` and `RoundRobinSelector` |
| EWMA | `Ewma` | `PolicyKind::Ewma` | Time-decayed Peak-EWMA latency score times outstanding load; prefers low-latency backends |
| LeastConn | `LeastConn` | `PolicyKind::LeastConn` | P2C using active-connection counts, not a global minimum scan |
| ConsistentHash | `ConsistentHash` (+ `consistentHash.hashOn`/`key`) | `PolicyKind::ConsistentHash` | Consistent hashing on a header / cookie / query-param value or normalized source IP |
| WeightedSelector | — | — | A general-purpose weighted selector implemented by `BackendSelector<T>`, used for selection across multiple BackendRefs (weight-based, not an LB algorithm choice) |

`PolicyKind` (`edgion-gateway/src/lb/compiled_selector.rs`) is the gateway's
own request-path selector-variant enum; `ParsedLBPolicy`
(`edgion-resources/src/resources/lb_policy.rs`) is the resource-schema-facing
runtime type produced by the CRD. `backends/selector.rs::policy_to_kind` maps
one onto the other.

## LB policy resolution

- The variant for a given Service is produced by
  `EdgionBackendTrafficPolicy`'s `LoadBalancerConfig::to_parsed()`
  (`edgion-resources/src/resources/edgion_backend_traffic_policy.rs`), mapping
  the CRD's `loadBalancer.type` onto `ParsedLBPolicy`
  (`edgion-resources/src/resources/lb_policy.rs`) — no string parsing on this
  path, and no per-route carrier anymore.
- The gateway's `BackendTrafficPolicyStore`
  (`backends/policy/backend_traffic/store.rs`) resolves the oldest-wins
  winning policy per typed Service key and publishes `load_balancer` on the
  immutable effective snapshot. HTTP/gRPC selection reads the request-captured
  `ParsedLBPolicy`, not a live store helper. The backend selector
  (`backends/selector.rs::policy_to_kind`) maps that policy onto the
  request-path `PolicyKind`. Every available Service generation already owns
  all four compiled variants; selection chooses one without compiling or
  hydrating on the request path.
- When not configured (no policy, or `loadBalancer.type: RoundRobin`),
  RoundRobin is used by default — the captured `load_balancer` is `None`.

## EWMA and LeastConn selection algorithm

EWMA and LeastConn use power-of-two choices (P2C): sample two distinct candidates,
read their metrics, then check eligibility of the lower-metric candidate followed
by the other candidate if needed. Equal metrics prefer the first sampled candidate.
LeastConn compares atomic active-connection counts. EWMA compares the
time-decayed Peak-EWMA latency estimate times outstanding load
(`latency estimate × (in-flight + 1)`). The estimator, sample-point, and useful-work
classification live in `edgion-gateway/src/lb/ewma/score.rs` and
`edgion-gateway/src/lb/sample.rs`; the math and invariants are in
[12-algorithms/lb-peak-ewma.md](../../12-algorithms/lb-peak-ewma.md).
The compiled selector permits up to 256 sampled pairs. If none passes eligibility,
a final linear scan checks eligibility without reading metrics. Empty and singleton
lists have direct paths. The sampler borrows candidates and clones only the winning
Arc; it does not build a per-request eligible-backend collection.

Native HTTP/gRPC Service selection uses the captured endpoint policy. TCP, UDP and
TLS passthrough Service paths call `select_service_endpoint` with `&None`, selecting
RoundRobin, and use `ServiceDegradePolicy::Live`. The retained TLS termination
implementation uses the same call shape, but TLSRoute Terminate remains disabled
at product admission; this is not an additional supported traffic mode.

## BackendSelector

`BackendSelector<T>` is a generic backend selector for weighted selection across multiple BackendRefs in a Route:

```rust
pub struct BackendSelector<T> {
    state: SelectorState<T>,
}

enum SelectorState<T> {
    Error(u32),                    // configuration error (no BackendRef or inconsistent weights)
    Single(T),                     // single backend, returned directly
    Multiple(WeightedRoundRobin<T>), // multiple backends, weighted round-robin
}
```

Characteristics:
- Route compilation constructs the complete plan with `BackendSelector::new(items, weights)`
  before publication. Private immutable state has no initialization or replacement API;
  empty inputs capture the existing no-backend error.
- HTTP/gRPC rule units share one `Arc<BackendSelector<usize>>` across matches and
  hostname indexes. L4 owns its selector in `CompiledL4Backends`. WRR's atomic cursor
  remains shared within that owner; new compiled generations receive independent
  cursors, while captured or reused old units retain their existing cursor.
- Selection reads the plan directly. Outer route snapshots own publication; no inner
  swap, request initialization, additional lock or request copy is needed.
- Weight validation: weights must be configured for all or for none; nonpositive weights
  are filtered out. HTTP/gRPC builders default omitted weights to 1. Original slot
  indices, invalid/denied weighted shares and HTTP AI dispatch classification are preserved.
- Error codes include `ERR_NO_BACKEND_REFS`, `ERR_INCONSISTENT_WEIGHT`, and
  `ERR_WRR_BUILD_FAILED`.

## Backend selection flow

```
HTTPRoute rule
  └── BackendRef[] (with optional weight)
       │
       ├── BackendSelector.select()  →  select target BackendRef
       │
       └── Resolve service_key (namespace/name)
            │
            ├── Capture the published Service projection and active membership
            │
            ├── Apply LB strategy to select a specific backend
            │   ├── Default: RrCore -> RoundRobinSelector
            │   ├── ConsistentHash: hash by request feature
            │   ├── LeastConn: P2C connection counts
            │   └── EWMA: P2C Peak-EWMA/load scores
            │
            │   Candidate eligibility is checked inside selection, using the
            │   captured Service runtime snapshot and live health/outlier state.
            │   The degrade branch explicitly bypasses that candidate predicate.
            │
            └── Return HttpPeer (IP:Port + TLS configuration)
```

## LB configuration

Attach an `EdgionBackendTrafficPolicy` to the Service (not the route):

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: my-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: my-service
  loadBalancer:
    type: LeastConn   # RoundRobin | LeastConn | Ewma | ConsistentHash
```

`ParsedLBPolicy` (`edgion-resources/src/resources/lb_policy.rs`) has three
non-default variants:
- `ConsistentHash(ConsistentHashOn)` — specifies the hash source (`Header` /
  `Cookie` / `Arg` / `SourceIp`, from `loadBalancer.consistentHash.hashOn`).
- `LeastConn`.
- `Ewma`.

Native HTTP/gRPC requests targeting the same Service and capturing the same
effective policy generation use the same endpoint algorithm. There is no per-route
algorithm override or union-of-policies semantics (the former route-fed
`PolicyStore` with
ref-counting is gone). See
[Behavior Changes](../../02-features/03-resources/20-edgion-backend-traffic-policy.md#behavior-changes-from-the-removed-mechanisms)
in the resource schema doc.

## Eager Service selectors

The Service/EndpointSlice Coordinator builds RoundRobin, ConsistentHash, LeastConn, and EWMA
selectors eagerly from the ready backends of each valid canonical section. Aliased
section keys reuse the same materialized section; there is no legacy unsectioned
selector projection. A semantic source no-op retains the complete projection Arc
and does not publish a new generation. Startup has no route scan or preload step, and request selection never populates a
selector cache.

## Directory layout

```
edgion-gateway/src/lb/
├── mod.rs                    # module exports
├── attempt.rs                # LbCommitGuard — outstanding-connection accounting
├── sample.rs                 # response -> latency-sample classification
├── backend_selector/         # general-purpose backend selector
│   ├── selector.rs           # BackendSelector<T> implementation
│   └── weighted_selector.rs  # WeightedRoundRobin<T> implementation
├── selection/                # strategy implementations
│   ├── round_robin.rs        # RoundRobin selection
│   ├── consistent_hash.rs    # consistent hashing selection
│   └── p2c.rs                # power-of-two-choices sampler (LeastConn and Ewma)
├── ewma/
│   └── score.rs              # PeakEwmaScore — packed, time-decayed Peak-EWMA
├── leastconn/                # LeastConn state management
│   └── cleaner.rs            # stale-state cleaner
├── compiled_selector.rs      # immutable eager selector bundle (multi-PolicyKind)
├── backend.rs                # LbBackend — LB-visible backend handle
└── runtime_state/            # runtime backend state
    └── mod.rs                # backend active-state tracking
```

The resolved LB policy itself is not stored under `lb/` — it lives in
`backends/policy/backend_traffic/store.rs` (`BackendTrafficPolicyStore`), the
`EdgionBackendTrafficPolicy` ConfHandler's arbitration store (see
[07-backends.md](07-backends.md#backend-traffic-policy-edgionbackendtrafficpolicy)).
The former `lb/lb_policy/` module (`LbPolicy` enum, the route-fed `PolicyStore`
with route ref-counting, and `routes/http/lb_policy_sync.rs`) was deleted
along with the route-level `LBPolicy*` `ExtensionRef` mechanism.
