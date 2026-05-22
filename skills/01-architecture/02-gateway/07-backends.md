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
│   ├── probes.rs         # Probe implementations (HTTP/TCP)
│   ├── config_store.rs   # HealthCheckConfigStore — health-check configuration store
│   ├── status_store.rs   # HealthStatusStore — health status store
│   └── annotation.rs     # Service annotation parsing
└── mod.rs
```

### HealthCheckManager

- Global singleton (`HEALTH_CHECK_MANAGER`, LazyLock).
- Maintains one background tokio task (`JoinHandle<()>`) per service.
- `reconcile_service(service_key)` — creates/updates/deletes the check task based on configuration.
- Configuration source priority: parsed result in HealthCheckConfigStore (annotation or CRD).
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
├── config.rs     # CircuitBreakerConfig — config struct + annotation parsing
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

- During Service sync, parse the `edgion.io/circuit-breaker` annotation → `set_service_config`.
- During EP/EPS `cleanup_removed_backends` → `remove_backend` cleans ejection state.
- On Service deletion → `remove_service` clears the entire CB configuration.

### Prometheus metrics

| Metric | Type | Label | Recorded when |
|--------|------|-------|---------------|
| `edgion_circuit_breaker_ejections_total` | Counter | `service` | An ejection succeeds |
| `edgion_circuit_breaker_ejected_endpoints` | Gauge | `service` | +1 on ejection, -1 on recovery / cleanup |
| `edgion_circuit_breaker_rejections_total` | Counter | `service` | LB selection hits an ejected backend |

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

1. Iterate all route kinds (HTTP/gRPC/TCP/UDP/TLS) and collect (service_key, lb_policy) pairs.
2. Deduplicate using HashSet.
3. Pre-create RoundRobin LB instances by calling `get_or_create()` on the EndpointSlice store.
4. Log the preload result (total, success, skipped).

## Endpoint validation

`validate_endpoint_in_route()` provides a security check:

- Verifies that the target IP is a legitimate endpoint within the route's BackendRefs.
- Prevents SSRF: ensures requests can only be routed to endpoints the route has authorized.
- Parameters: target IP, optional port, the route's backend_refs, the route's namespace.
- Returns: `Ok((backend_ref_index, port))` or `Err(reason)`.
- Used by plugins such as `DirectEndpoint` and `DynamicInternalUpstream`.

## Directory layout

```
src/core/gateway/backends/
├── mod.rs                    # module exports
├── discovery/                # backend discovery
│   ├── endpoint_slice/       # EndpointSlice discovery
│   └── services/             # Service metadata
├── health/                   # health checks
│   └── check/                # check system
├── circuit_breaker/          # circuit breaker (passive ejection)
├── policy/                   # backend policies
│   └── backend_tls/          # BackendTLSPolicy (upstream mTLS)
├── preload.rs                # LB preload
└── validation.rs             # endpoint validation (SSRF prevention)
```
