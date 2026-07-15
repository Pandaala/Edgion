---
name: gateway-backends
description: Backend discovery and health checks — three discovery sources, BackendStore, the health-check system, BackendTLSPolicy, preload, and endpoint validation.
---

# Backend discovery and health checks

> The backend module is responsible for fetching the actual Pod endpoints behind a Service from Kubernetes,
> and for using the health-check system to automatically eject unhealthy backends and reinstate healthy ones.

## Discovery sources

| Source | Module | K8s resource | Description |
|--------|--------|--------------|-------------|
| EndpointSlice | `discovery/endpoint_slice/` | `EndpointSlice` | Backend endpoint discovery |
| Service | `discovery/services/` | `Service` | Service metadata storage (ClusterIP, port mapping) |

### Discovery data flow

```
Controller (K8s Watch)
  │
  ├── gRPC ConfigSync
  │
  ├── ConfHandler<EndpointSlice>  →  EpSliceStore
  │                                    └── RoundRobin Store (DashMap)
  │                                         └── Pingora LoadBalancer (per service_key)
  │
  └── ConfHandler<Service>        →  ServiceStore (DashMap)
                                       └── Service metadata (ports, annotations)
```

Each discovery source implements:
- `conf_handler_impl.rs` — ConfHandler trait implementation, handling full_set / partial_update.
- `discovery_impl.rs` — discovery logic, converting K8s resources into a Pingora Backend list.
- `*_store.rs` — data store (EpSliceStore / ServiceStore).

## Health-check system

### Component layout

```
health/
├── check/
│   ├── manager.rs        # HealthCheckManager — manages a check task per service
│   ├── probes.rs         # Probe implementations (HTTP/TCP/gRPC)
│   ├── config_store.rs   # HealthCheckConfigStore — health-check configuration store
│   └── status_store.rs   # HealthStatusStore — health status store
└── mod.rs
```

### HealthCheckManager

- Global singleton (`HEALTH_CHECK_MANAGER`, LazyLock).
- Maintains one background tokio task (`JoinHandle<()>`) per service.
- `reconcile_service(service_key)` — creates/updates/deletes the check task based on configuration.
- **Configuration source**: `HealthCheckConfigStore` is a single map keyed by
  Service key (`namespace/service`) — there are no source tiers anymore. The
  `EdgionBackendTrafficPolicy` policy store
  (`backends/policy/backend_traffic/store.rs`) is the **sole writer**: on every
  policy full_set/partial_update it resolves the oldest-wins winner per
  targeted Service key and calls `set_service_config` +
  `hc_manager.reconcile_service(key)`. There is no annotation-parsing path —
  `health/check/annotation.rs` and the EndpointSlice-tier override were
  removed; see [Backend Traffic Policy](#backend-traffic-policy-edgionbackendtrafficpolicy) below.
- Without configuration, deletes the task and unregisters health status.

### Probe types

| Type | Function | Description |
|------|----------|-------------|
| HTTP | `probe_http()` / `probe_http_with_client()` | HTTP GET check, supports a custom client |
| TCP | `probe_tcp()` | TCP connection check |
| gRPC | `probe_grpc()` | gRPC Health Check Protocol (grpc.health.v1.Health/Check), implemented with hand-written prost structs, no proto file needed |

### Health status

- `HealthStatusStore` stores the health status of each backend per service.
- Unhealthy backends are filtered out during LB selection (via the `health_filter` callback).
- Backends are automatically re-added to the selection pool after recovery.
- `unregister_service()` cleans up state when the check configuration is removed.

### backend_hash

Used to compute the key for health status and circuit-breaker state: a DefaultHasher hash of the Backend's `addr`.

## Circuit Breaker

Passive ejection mechanism to prevent cascading failures. References a slimmed-down version of Envoy's `outlier_detection`.

### Component layout

```
circuit_breaker/
├── config.rs     # re-exports CircuitBreakerConfig / OutlierDetectionConfig
│                 # (the struct + normalize()/validate() now live in
│                 # edgion-resources::resources::outlier_detection)
├── store.rs      # CircuitBreakerStore — DashMap-based ejection state store
└── mod.rs
```

### Core mechanics

- **Consecutive failure counting**: connect failures / proxy errors / upstream 5xx each count as one; non-5xx responses reset the counter.
- **Auto ejection**: once consecutive failures hit the threshold, mark `ejected_at`; LB selection skips the backend.
- **Auto recovery**: when `is_ejected()` is checked and the cooldown has expired, the ejection state is cleared automatically.
- **Maximum ejection ratio**: prevents all backends from being ejected at once (0 = no limit).

### Data store

`CircuitBreakerStore` (global LazyLock singleton):
- Outer `DashMap<String, Arc<ServiceCbState>>`: service_key → configuration + endpoint state.
- Inner `DashMap<u64, EndpointState>`: backend_hash → consecutive-failure count + ejection time.
- `record_success` deletes the entry directly (not reset to 0) to prevent memory growth.
- Services without CB configuration have no entry; `has_service()` returns false and skips immediately.

### Failure-counting hooks

| Hook | File | Trigger |
|------|------|---------|
| `fail_to_connect` | `pg_fail_to_connect.rs` | Connection failed |
| `error_while_proxy` | `pg_error_while_proxy.rs` | Proxy error (non retry-on-status) |
| `upstream_response_filter` | `pg_upstream_response_filter.rs` | 5xx records failure, non-5xx records success |

### LB integration

`is_backend_healthy()` adds a CB ejection check after the health check; it applies automatically to all LB algorithms.

### Lifecycle

- `CircuitBreakerStore.set_service_config` is written **only** by the
  `EdgionBackendTrafficPolicy` policy store (oldest-wins winner's
  `spec.outlierDetection`, normalized) — see
  [Backend Traffic Policy](#backend-traffic-policy-edgionbackendtrafficpolicy)
  below. There is no annotation parsing anymore.
- During EndpointSlice `cleanup_removed_backends` → `remove_backend` cleans
  per-backend ejection state (this stays owned by the Service/EndpointSlice
  handlers — only the per-service *policy config* moved).
- A Service/EndpointSlice `full_set` resync (e.g. every gateway reconnect)
  must **not** delete the per-service CB/HC/panic/LB policy config — that
  would wipe policy-written entries with nothing to re-apply them. Only the
  policy `ConfHandler` creates/updates/deletes those entries, independent of
  whether the Service currently exists.

### Prometheus metrics

| Metric | Type | Label | Recorded when |
|--------|------|-------|---------------|
| `edgion_circuit_breaker_ejections_total` | Counter | `service` | An ejection succeeds |
| `edgion_circuit_breaker_ejected_endpoints` | Gauge | `service` | +1 on ejection, -1 on recovery / cleanup |
| `edgion_circuit_breaker_rejections_total` | Counter | `service` | LB selection hits an ejected backend |

## Backend Traffic Policy (EdgionBackendTrafficPolicy)

Full external schema: [02-features/03-resources/20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md).

```
policy/
├── backend_traffic/
│   ├── store.rs             # BackendTrafficPolicyStore — synced cache + arbitration
│   ├── conf_handler_impl.rs # ConfHandler<EdgionBackendTrafficPolicy>
│   └── mod.rs                # resolve_lb_policy() — request-path LB lookup
└── mod.rs
```

`BackendTrafficPolicyStore` (`get_backend_traffic_policy_store()`, global
singleton) is the single owner of per-service *policy config* — it is the
**sole writer** of `HealthCheckConfigStore`, `CircuitBreakerStore`, the
`PanicThresholdStore` per-service override, and its own `lb_policies: DashMap<service_key, ParsedLBPolicy>`
(read on the request path via `resolve_lb_policy(service_key)` /
`backends::policy::resolve_lb_policy`). This replaced the former route-fed,
ref-counted `lb/lb_policy/PolicyStore` (deleted).

- **Arbitration** (`reapply_service(service_key)`): among all cached policies
  whose `targetRefs` include this Service key, skip specs that fail
  `get_validation_errors()`, then pick the minimum `precedence_key` (oldest
  `creationTimestamp`, then name) — same rule as `BackendTLSPolicy`. The
  winner's `loadBalancer` / `healthCheck` / `outlierDetection` sections are
  pushed atomically (whole-section, no per-field merge across policies) into
  the four stores; no winner clears all four.
- **`full_set_policies` / `partial_update_policies`**: `partial_update` is a
  real incremental diff (ConfHandler iron rule) — it recomputes only the
  Service keys touched by the changed policies' old ∪ new `targetRefs`.
- **EPS rebuild on LB change**: when `reapply_service` flips the applied LB
  policy for a key, it calls
  `config_client.trigger_endpoint_slice_update_event(service_key)` so
  `ServiceSelectorCell`s rebuild against the new algorithm.
- Admin `/store/stats` exposes the resolved LB map as `lb_policies` (a
  `service_key → algorithm` map) — this is the field that replaced the old
  `policy_store` stat.

## BackendTLSPolicy

Manages upstream mTLS configuration, stored in `BackendTLSPolicyStore`:

```
policy/
├── backend_tls/
│   ├── backend_tls_policy_store.rs  # BackendTLSPolicyStore (storage + queries)
│   ├── conf_handler_impl.rs         # ConfHandler implementation
│   └── mod.rs
└── mod.rs
```

- Get the global instance via `get_global_backend_tls_policy_store()`.
- During the `upstream_peer` phase, look up the TLS policy for the service.
- Configure the HttpPeer's TLS connection parameters (CA certificate, client certificate / key).
- Secret key constants: `CA_CERT`, `CERT`, `KEY`.

## LB preload

`preload_load_balancers()` is invoked after all configuration sync completes:

1. Iterate all route kinds (HTTP/gRPC/TCP/UDP/TLS) and collect referenced Service keys.
2. Deduplicate using HashSet.
3. Hydrate each `ServiceSelectorCell` backend list if empty; algorithm-specific compiled selectors
   remain lazy.
4. Log the preload result.

## Endpoint validation

`validate_endpoint_in_route()` provides a security check:

- Verifies that the target IP is a legitimate endpoint within the route's BackendRefs.
- Prevents SSRF: ensures requests can only be routed to endpoints the route has authorized.
- Parameters: target IP, optional port, the route's backend_refs, the route's namespace.
- Returns: `Ok((backend_ref_index, port))` or `Err(reason)`.
- Used by plugins such as `DirectEndpoint` and `DynamicInternalUpstream`.

## Directory layout

```
edgion-gateway/src/backends/
├── mod.rs                    # module exports
├── discovery/                # backend discovery
│   ├── endpoint_slice/       # EndpointSlice discovery
│   └── services/             # Service metadata
├── health/                   # health checks
│   └── check/                # check system
├── circuit_breaker/          # circuit breaker (passive ejection)
├── panic_threshold.rs        # panic-threshold store + resolution
├── policy/                   # backend policies
│   ├── backend_tls/          # BackendTLSPolicy (upstream mTLS)
│   └── backend_traffic/      # EdgionBackendTrafficPolicy (HC/CB/panic/LB arbitration)
├── preload.rs                # LB preload
└── validation.rs             # endpoint validation (SSRF prevention)
```
