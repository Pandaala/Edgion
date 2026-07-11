---
name: edgion-backend-traffic-policy-features
description: Full EdgionBackendTrafficPolicy schema — per-destination load balancing, active health check, and outlier detection attached to a Service via targetRefs.
---

# EdgionBackendTrafficPolicy Resource

> API: `edgion.io/v1` | Scope: Namespaced | Short name: `ebtp`
> Design: `docs/superpowers/specs/2026-07-10-edgion-backend-traffic-policy-design.md`

`EdgionBackendTrafficPolicy` is the single configuration surface for "how the
gateway connects to a backend Service": load-balancing algorithm (including
consistent hash and the panic threshold), active health checks, and outlier
detection (passive ejection). It attaches to one or more Services in the
**same namespace** via `spec.targetRefs`, following the Gateway API
policy-attachment pattern (the same `DirectSameNs` semantics as
[BackendTLSPolicy](12-backend-tls-policy.md)).

It replaces three previously separate mechanisms, all removed: the
`edgion.io/health-check` and `edgion.io/circuit-breaker` /
`edgion.io/outlier-detection` Service (and EndpointSlice) annotations, the
`edgion.io/panic-threshold` Service annotation, and the route-level
`LBPolicy*` `ExtensionRef` filters. `BackendTLSPolicy` (upstream TLS/mTLS) is
**not** absorbed and stays a separate resource.

## Full Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: payments-policy
  namespace: prod
spec:
  targetRefs:
    - kind: Service          # v1: only Service (group "" / core)
      name: payments

  loadBalancer:               # optional; absent = RoundRobin, no panic override
    type: LeastConn           # RoundRobin | LeastConn | Ewma | ConsistentHash
    consistentHash:            # required iff type == ConsistentHash
      hashOn: header           # header | cookie | queryParam
      key: x-user-id
    panicThreshold: 30         # 0-100; overrides the global
                                # EdgionGatewayConfig.spec.loadBalancing.panicThreshold

  healthCheck:                 # optional; absent = no active probing
    active:
      type: http                # http | tcp | grpc
      path: /healthz
      port: 8080
      interval: 10s
      timeout: 3s
      healthyThreshold: 2
      unhealthyThreshold: 3
      expectedStatuses: [200]
      host: payments.internal
      grpcServiceName: ""

  outlierDetection:            # optional; absent = no passive ejection
    consecutiveErrors: 5
    consecutiveGatewayErrors: 5
    consecutiveLocalOriginFailures: 5
    ejectionSeconds: 30
    maxEjectionSeconds: 300
    maxEjectionPercent: 50
status:
  conditions: []                # Accepted / ResolvedRefs / Conflicted
```

All four spec sections (`loadBalancer`, `healthCheck`, `outlierDetection`, and
each field within them) are optional; an absent section means "default
behavior" for that concern (see [Defaults](#defaults-when-no-policy-applies)
below). Sections are **atomic**: whichever policy wins for a Service (see
[Conflict resolution](#conflict-resolution)) provides the whole section — there
is no per-field merge across policies.

## spec Field Details

### targetRefs

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `group` | `String` | No | Empty string (default) = core API group. Only the core group is supported in v1. |
| `kind` | `String` | Yes | Only `Service` is supported in v1. |
| `name` | `String` | Yes | Service name, same namespace as the policy. |

### loadBalancer

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | Enum | Yes (if section present) | `RoundRobin` \| `LeastConn` \| `Ewma` \| `ConsistentHash`. `RoundRobin` is the engine default and needs no explicit `consistentHash`. |
| `consistentHash` | Object | Only when `type: ConsistentHash` | `hashOn` (`header` \| `cookie` \| `queryParam`) + `key` (header/cookie/query-param name). Rejected (Accepted=False) when present with any other `type`, or absent when `type: ConsistentHash`. |
| `panicThreshold` | `u8` | No | `0`-`100`. Overrides `EdgionGatewayConfig.spec.loadBalancing.panicThreshold` for the target Service(s). Values > 100 are rejected. |

`consistentHash.hashOn` maps onto the gateway's `ConsistentHashOn` runtime enum
(`edgion-resources/src/resources/lb_policy.rs`): `header` → `Header`, `cookie`
→ `Cookie`, `queryParam` → `Arg`. The field is deliberately **not** named `on`
— an unquoted `on` is a YAML 1.1 boolean, and `sigs.k8s.io/yaml` (kubectl's
converter) would turn the key into `"true"`, breaking structural validation.

### healthCheck

Reuses the existing `ServiceHealthCheck` / `ActiveHealthCheckConfig` shape
unchanged (same fields as the pre-existing health-check annotation payload) —
see `edgion-resources/src/resources/health_check.rs`. Validation
(`ActiveHealthCheckConfig`'s own rules: thresholds ≥ 1, non-zero
interval/timeout, non-empty `path` and `expectedStatuses` for `http`) is
unchanged.

### outlierDetection

Mirrors the pre-existing circuit-breaker annotation payload — see
`edgion-resources/src/resources/outlier_detection.rs`
(`OutlierDetectionConfig`). Three independent failure counters
(`consecutiveErrors` = application 5xx, `consecutiveGatewayErrors` = upstream
502/503/504, `consecutiveLocalOriginFailures` = connect/TLS/transport
failures); any one reaching its threshold ejects the endpoint.
`consecutiveGatewayErrors`, `consecutiveLocalOriginFailures`, and
`maxEjectionSeconds` are sibling-dependent defaults (`consecutiveErrors`,
the resolved `consecutiveGatewayErrors`, and `max(300, ejectionSeconds)`
respectively) resolved by `OutlierDetectionConfig::normalize()` in code — the
CRD YAML deliberately does **not** declare OpenAPI defaults for those three
fields, since a naive schema default would short-circuit the cross-field
resolution.

## Defaults When No Policy Applies

| Concern | Default (no policy, or no matching section) |
|---------|-----------------------------------------------|
| Health check | None — no probe task runs |
| Outlier detection | None — no ejection tracking |
| Panic threshold | The global `EdgionGatewayConfig.spec.loadBalancing.panicThreshold` (itself `0` = disabled by default) |
| LB algorithm | `RoundRobin` |

**When to set `panicThreshold`.** The engine-wide default is `0` (disabled),
matching Istio's Kubernetes default, because a 50% threshold misfires on small
pools — a 2-pod service losing 1 pod hits 50% and would route back to the bad
pod. Set it (e.g. `50`) via this policy's `spec.loadBalancer.panicThreshold`
for services with **larger** backend pools (≈ ≥ 5 endpoints), where losing
many backends at once more likely means a systemic problem (a misconfigured
health-check path, a network partition) than genuine failure. The canonical
case: a renamed health-check endpoint marks every backend unhealthy and,
without panic mode, takes the whole service offline with 503s.

## Conflict Resolution

When multiple `EdgionBackendTrafficPolicy` resources target the same Service,
**oldest `creationTimestamp` wins**:

1. Sort by `creationTimestamp` ascending (earlier-created wins).
2. A policy with no `creationTimestamp` (the file-system conf center produces
   `None`) loses to any timestamped policy.
3. Remaining ties break by resource name, lexicographic.

Both the Controller (status computation) and the gateway `ConfHandler` (store
writes) apply the identical rule
(`edgion_resources::resources::edgion_backend_traffic_policy::precedence_key`).
The losing policy gets `Conflicted=True`; deleting the winner promotes the
next-oldest loser on the next reconcile. This is the same pattern
`BackendTLSPolicy` uses (`handlers/backend_tls_policy.rs`), scoped per target
Service key instead of per `(Service, sectionName)`.

## Status Schema

```yaml
status:
  conditions:
    - type: Accepted
      status: "True"
      reason: Accepted
    - type: ResolvedRefs
      status: "True"
      reason: ResolvedRefs
    - type: Conflicted
      status: "False"
      reason: NoConflicts
```

### Accepted condition reasons

| Reason | Meaning |
|--------|---------|
| `Accepted` | Spec validation passed. |
| `Invalid` | `spec.get_validation_errors()` is non-empty (bad `targetRefs`, `consistentHash` mismatch, `panicThreshold` out of range, invalid `healthCheck` / `outlierDetection`). |

Following the lenient-admission pattern used by `EdgionPlugins`, an
`Accepted=False` policy still syncs to the gateway — the gateway `ConfHandler`
re-runs the same validation and skips invalid specs as defense in depth, so an
invalid policy never reaches the runtime stores.

### ResolvedRefs condition reasons

| Reason | Meaning |
|--------|---------|
| `ResolvedRefs` | All target Services exist in the current Service cache. |
| `ServiceNotFound` | At least one `targetRefs` Service does not currently exist. The policy still syncs and takes effect the moment the Service appears — a Service create/delete requeues the policy. |

### Conflicted condition reasons

| Reason | Meaning |
|--------|---------|
| `NoConflicts` | This policy is the current winner (or has sole ownership) for every targeted Service. |
| `LostOldestWins` | This policy lost the oldest-wins arbitration for at least one target Service. |

## Examples

### Least-connection LB with active health check

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: api-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: api-service
  loadBalancer:
    type: LeastConn
  healthCheck:
    active:
      type: http
      path: /healthz
      interval: 10s
      timeout: 3s
```

### Consistent hash by cookie, with outlier detection

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: cache-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: cache-service
  loadBalancer:
    type: ConsistentHash
    consistentHash:
      hashOn: cookie
      key: session_id
  outlierDetection:
    consecutiveErrors: 5
    ejectionSeconds: 30
    maxEjectionPercent: 50
```

### Panic-threshold override only

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: large-pool-panic
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: fanout-service
  loadBalancer:
    type: RoundRobin
    panicThreshold: 50
```

## Behavior Changes From The Removed Mechanisms

- **Per-route LB is gone.** Before this resource, the LB algorithm was carried
  per `HTTPRoute`/`GRPCRoute` `backendRef` via an `ExtensionRef` filter
  (`LBPolicyConsistentHash`, `LBPolicyLeastConn`, `LBPolicyEwma`), so two
  routes to the same Service could use different algorithms. LB policy is now
  strictly **per-destination**: one algorithm per Service, matching the Istio
  `DestinationRule` model. A route that still references an `LBPolicy*`
  `ExtensionRef` fails validation like any other unrecognized `extensionRef`
  kind.
- Health check, outlier detection, and panic threshold move from
  Service/EndpointSlice annotations to this CRD; the former EndpointSlice-tier
  override (higher priority than the Service tier) has no replacement — the
  policy attaches only at the Service level.
- Not in scope for v1: static IP / external-hostname targets, connection-pool
  parameters (still route annotations), cross-namespace `targetRefs`,
  port-level (`sectionName`) overrides, and policy-driven LB for TCP/UDP/TLS
  routes (they remain fixed RoundRobin).

## Architecture / Data-Plane Flow

Internal processing flow (Controller `ProcessorHandler`, gateway
`ConfHandler`, and the runtime stores) lives in
[01-architecture/02-gateway/07-backends.md](../../01-architecture/02-gateway/07-backends.md)
(health check + outlier detection) and
[01-architecture/02-gateway/06-load-balancing.md](../../01-architecture/02-gateway/06-load-balancing.md)
(LB algorithm resolution) — this page covers the external schema only.

## Related Documentation

- [16-service-backends.md](16-service-backends.md) — Service/EndpointSlice discovery this policy attaches to.
- [12-backend-tls-policy.md](12-backend-tls-policy.md) — the sibling policy-attachment resource for upstream TLS (not absorbed by this CRD).
- User-facing how-to: `docs/en/user-guide/http-route/backends/health-check.md`, `circuit-breaker.md`, `docs/en/user-guide/http-route/lb-algorithms.md`.
