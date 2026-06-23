---
name: gateway-load-balancing
description: Load balancing strategies — RoundRobin, Random, EWMA, LeastConn, ConsistentHash, WeightedSelector, and the backend selection flow.
---

# Load balancing

> The load balancing module is responsible for selecting a target instance from the backend list.
> The default is weighted round-robin (WeightedRoundRobin); a LoadBalancer policy can be specified via the HTTPRoute ExtensionRef filter.

## Load balancing strategies

| Strategy | Type | Configuration alias | Description |
|----------|------|---------------------|-------------|
| RoundRobin | default | (default, no configuration needed) | Weighted round-robin, implemented via `WeightedRoundRobin<T>` |
| Random | built-in | (random selection) | Random backend selection |
| EWMA | `LbPolicy::Ewma` | `"ewma"` | Exponentially weighted moving average of response time, prefers low-latency backends |
| LeastConn | `LbPolicy::LeastConnection` | `"leastconn"`, `"least-connection"`, `"leastconnection"`, `"least_connection"` | Least connections, prefers the backend with the fewest active connections |
| ConsistentHash | `LbPolicy::Consistent` | `"consistent"`, `"consistent-hash"`, `"ketama"` | Consistent hashing, supports computing the hash from header/cookie/IP/query param, etc. |
| WeightedSelector | — | — | A general-purpose weighted selector implemented by `BackendSelector<T>`, used for selection across multiple BackendRefs |

## LB policy parsing

The `LbPolicy` enum defines three non-default strategies:

```rust
pub enum LbPolicy {
    Consistent,       // consistent hashing
    LeastConnection,  // least connections
    Ewma,             // exponentially weighted moving average
}
```

- The variant for a given backend is produced inside `lb_policy_sync.rs` by mapping
  `ParsedLBPolicy` (the typed extension) onto `LbPolicy` — no string parsing on this path.
- When not configured, RoundRobin is used by default.

## EWMA and LeastConn selection algorithm

EWMA and LeastConn share the `select_by_min_metric()` algorithm:

1. Build a min-heap (BinaryHeap + Reverse) sorted by metric value.
2. Skip inactive backends (checked via `runtime_state::is_backend_active()`).
3. Pop candidates from the heap one by one and check health via the `health_filter`.
4. Return the first backend with the lowest metric that passes the health check.
5. Iterate at most `max_iterations` times.

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
- Lazy initialization: initialized on first use via `init(items, weights)`.
- Thread-safe: uses ArcSwap internally.
- Weight validation: weights must be configured for all or for none; backends with weight 0 are filtered out.
- Error codes: `ERR_NO_BACKEND_REFS`(1) and `ERR_INCONSISTENT_WEIGHT`(2).

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

Reference a `LoadBalancer` resource via the HTTPRoute / GRPCRoute `ExtensionRef` filter:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
spec:
  rules:
    - backendRefs:
        - name: my-service
          port: 8080
      filters:
        - type: ExtensionRef
          extensionRef:
            group: gateway.edgion.io
            kind: LoadBalancer
            name: my-lb-policy
```

`ParsedLBPolicy` is parsed and stored in the `extension_info` field of the BackendRef. Supported entries:
- `ConsistentHash(ConsistentHashOn)` — specifies the hash source.
- `LeastConn`.
- `Ewma`.

## LB Preload (warmup)

`preload_load_balancers()` is called after startup completes (after `wait_for_ready()`) to reduce first-request latency:

1. Collect (service_key, lb_policy) pairs referenced by all route kinds (HTTP/gRPC/TCP/UDP/TLS).
2. Deduplicate using HashSet.
3. For each key, call `get_or_create()` to pre-create a RoundRobin LB instance.
4. LeastConn / EWMA / ConsistentHash read RR's backend list; no separate pre-creation is needed.

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
├── lb_policy/                # LB policy configuration
│   ├── types.rs              # LbPolicy enum definition
│   ├── config.rs             # policy configuration parsing
│   └── policy_store.rs       # policy store
└── runtime_state/            # runtime backend state
    └── mod.rs                # backend active-state tracking
```
