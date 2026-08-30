---
name: gateway-load-balancing
description: Load balancing strategies — RoundRobin, EWMA, LeastConn, ConsistentHash, WeightedSelector, and the backend selection flow; algorithm source is EdgionBackendTrafficPolicy.
---

# Load balancing

> The load balancing module is responsible for selecting a target instance from the backend list.
> The default is weighted round-robin (WeightedRoundRobin); a non-default algorithm is configured
> **per-destination** via the `EdgionBackendTrafficPolicy` CRD attached to the Service (not per-route).
> Full external schema: [02-features/03-resources/20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md).

## Load balancing strategies

| Strategy | CRD `loadBalancer.type` | Selector-side `PolicyKind` | Description |
|----------|--------------------------|------------------------------|-------------|
| RoundRobin | `RoundRobin` (default; section may be omitted entirely) | `PolicyKind::RoundRobin` | Weighted round-robin, implemented via `WeightedRoundRobin<T>` |
| EWMA | `Ewma` | `PolicyKind::Ewma` | Exponentially weighted moving average of response time, prefers low-latency backends |
| LeastConn | `LeastConn` | `PolicyKind::LeastConn` | Least connections, prefers the backend with the fewest active connections |
| ConsistentHash | `ConsistentHash` (+ `consistentHash.hashOn`/`key`) | `PolicyKind::ConsistentHash` | Consistent hashing on a header / cookie / query-param value |
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
  winning policy per Service key and caches the result in its
  `lb_policies: DashMap<service_key, ParsedLBPolicy>`. The backend selector
  (`backends/selector.rs::policy_to_kind`) maps `ParsedLBPolicy` onto the
  request-path `PolicyKind` used by `ServiceSelectorCell`.
- When not configured (no policy, or `loadBalancer.type: RoundRobin`),
  RoundRobin is used by default — `resolve_lb_policy(service_key)` returns
  `None`.

## EWMA and LeastConn selection algorithm

EWMA and LeastConn use power-of-two choices (P2C): sample two eligible backends and select the
lower-metric candidate. LeastConn compares atomic active-connection counts; EWMA compares the
atomic latency score. Repeated bounded sampling applies health/activity filtering without scanning
or heap-building across the entire backend set.

## BackendSelector

`BackendSelector<T>` is a generic backend selector for weighted selection across multiple BackendRefs in a Route:

```rust
pub struct BackendSelector<T> {
    state: ArcSwap<Option<SelectorState<T>>>,
}

enum SelectorState<T> {
    Error(u32),                    // configuration error (no BackendRef or inconsistent weights)
    Single(T),                     // single backend, returned directly
    Multiple(WeightedRoundRobin<T>), // multiple backends, weighted round-robin
}
```

Characteristics:
- Route compilation initializes the selector with `init(items, weights)` before the unit is published;
  request handling only selects from the compiled state.
- Thread-safe: uses ArcSwap internally.
- Weight validation: weights must be configured for all or for none; backends with weight 0 are filtered out.
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
            ├── Get backend list (Backend[]) from EndpointSlice/Endpoint store
            │
            ├── Apply LB strategy to select a specific backend
            │   ├── Default: RoundRobin (Pingora LoadBalancer)
            │   ├── ConsistentHash: hash by request feature
            │   ├── LeastConn: least connections
            │   └── EWMA: lowest latency
            │
            ├── Health check filtering (Pingora Backend health)
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
  `Cookie` / `Arg`, from `loadBalancer.consistentHash.hashOn`).
- `LeastConn`.
- `Ewma`.

Because policy is attached per-Service, two routes pointing at the same
Service always share one algorithm — there is no per-route override and no
union-of-policies semantics (the former route-fed `PolicyStore` with
ref-counting is gone). See
[Behavior Changes](../../02-features/03-resources/20-edgion-backend-traffic-policy.md#behavior-changes-from-the-removed-mechanisms)
in the resource schema doc.

## LB Preload (warmup)

`preload_load_balancers()` is called after startup completes (after `wait_for_ready()`) to reduce first-request latency:

1. Collect service_key values referenced by all route kinds (HTTP/gRPC/TCP/UDP/TLS) — it never
   consumed LB policy, only the service key set, so it needed no change when LB policy moved off
   the route.
2. Deduplicate using HashSet.
3. For each key, hydrate the `ServiceSelectorCell` backend list if it is empty.
4. Algorithm-specific compiled selectors remain lazy and are built by `get_or_build` when that
   policy is first selected. LeastConn / EWMA / ConsistentHash resolve the effective
   `EdgionBackendTrafficPolicy` at selection time.

## Directory layout

```
edgion-gateway/src/lb/
├── mod.rs                    # module exports
├── backend_selector/         # general-purpose backend selector
│   ├── selector.rs           # BackendSelector<T> implementation
│   └── weighted_selector.rs  # WeightedRoundRobin<T> implementation
├── selection/                # strategy implementations
│   ├── round_robin.rs        # RoundRobin selection
│   ├── consistent_hash.rs    # consistent hashing selection
│   ├── ewma.rs               # EWMA selection
│   └── least_conn.rs         # least-connection selection
├── ewma/                     # EWMA metric tracking
│   └── metrics.rs            # EWMA latency metrics
├── leastconn/                # LeastConn state management
│   ├── backend_state.rs      # backend connection state
│   └── cleaner.rs            # stale-state cleaner
├── compiled_selector.rs      # ServiceSelectorCell — per-service selector cell (multi-PolicyKind)
├── edgion_backend.rs         # EdgionBackend — LB-visible backend handle
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
