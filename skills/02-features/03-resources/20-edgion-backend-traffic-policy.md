---
name: edgion-backend-traffic-policy-features
description: Full EdgionBackendTrafficPolicy schema — per-destination load balancing, active health check, outlier detection, and dynamic upstream authority (Host/SNI) attached to a Service via targetRefs.
---

# EdgionBackendTrafficPolicy Resource

> API: `edgion.io/v1` | Scope: Namespaced | Short name: `ebtp`
> Design: `docs/superpowers/specs/2026-07-10-edgion-backend-traffic-policy-design.md`,
> `docs/superpowers/specs/2026-07-14-dynamic-upstream-authority-design.md` (`upstreamAuthority`)

`EdgionBackendTrafficPolicy` is the single configuration surface for "how the
gateway connects to a backend Service": load-balancing algorithm (including
consistent hash and the panic threshold), active health checks, outlier
detection (passive ejection), and dynamic upstream authority (per-request
`Host`/SNI on a fixed dial target — see
[`upstreamAuthority`](#upstreamauthority)). It attaches to one or more
Services in the **same namespace** via `spec.targetRefs`, following the
Gateway API policy-attachment pattern (the same `DirectSameNs` semantics as
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
      hashOn: header           # header | cookie | queryParam | sourceIp
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

  upstreamAuthority:            # optional; absent = today's static-SNI behavior
    pattern: "*.sandbox.internal"                # exactly one leading "*." wildcard label
    template: "${ctx:tenant}.sandbox.internal"    # required; literal tail must equal
                                                   # the pattern's suffix
    healthCheckHost: "probe.sandbox.internal"     # required iff healthCheck.active is set
status:
  conditions: []                # Accepted / ResolvedRefs / Conflicted
```

All five spec sections (`loadBalancer`, `healthCheck`, `outlierDetection`,
`upstreamAuthority`, and each field within them) are optional; an absent
section means "default behavior" for that concern (see
[Defaults](#defaults-when-no-policy-applies) below). Sections are **atomic**:
whichever policy wins for a Service (see [Conflict resolution](#conflict-resolution))
provides the whole section — there is no per-field merge across policies.

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
| `consistentHash` | Object | Only when `type: ConsistentHash` | `hashOn` (`header` \| `cookie` \| `queryParam` \| `sourceIp`) + `key` (header/cookie/query-param name; **omit for `sourceIp`**). For `sourceIp` a present `key` is rejected (Accepted=False); for the other three an absent/empty `key` is rejected. The whole section is rejected when present with any other `type`, or absent when `type: ConsistentHash`. |
| `panicThreshold` | `u8` | No | `0`-`100`. Overrides `EdgionGatewayConfig.spec.loadBalancing.panicThreshold` for the target Service(s). Values > 100 are rejected. |

`consistentHash.hashOn` maps onto the gateway's `ConsistentHashOn` runtime enum
(`edgion-resources/src/resources/lb_policy.rs`): `header` → `Header`, `cookie`
→ `Cookie`, `queryParam` → `Arg`, `sourceIp` → `SourceIp` (a no-argument variant
that hashes on the RealIp-normalized client source IP). The field is deliberately
**not** named `on`
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

### upstreamAuthority

Lets a Service that dispatches on virtual host — e.g. a multi-tenant sandbox
backend serving `tenant-a.sandbox.internal`, `tenant-b.sandbox.internal`, …
behind one wildcard certificate — vary the upstream `Host` header and TLS SNI
**per request**, while the dial target stays pinned to whatever the Service
resolves to. Both the standard backend path (HTTPRoute/GRPCRoute → Service)
and the outbound path (Webhook LinkSys / `forward_auth`, when their `target`
names the same Service) read this one section; see
`docs/superpowers/specs/2026-07-14-dynamic-upstream-authority-design.md` for
the full design and safety argument.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `pattern` | `String` | Yes (if section present) | Exactly one leading `*.` wildcard label followed by a non-empty literal suffix, e.g. `*.sandbox.internal`. This is simultaneously the allowlist and the shape every render must fit. |
| `template` | `String` | Yes | How the wildcard label is produced per request, e.g. `${ctx:tenant}.sandbox.internal`. Its literal tail must equal `pattern`'s suffix exactly, and the remaining prefix — the label slot — must contain no literal `.` (it may still mix literals and `${...}` expressions, e.g. `tenant-${ctx:id}`). Rejected (`Accepted=False`) otherwise. |
| `healthCheckHost` | `String` | Required whenever `healthCheck.active` is configured on the same policy | Static authority the active health probe sends, since a probe has no request to render a label from. Must itself match `pattern` (`Accepted=False` otherwise). |

**How it works.** Only the wildcard **label** is rendered from request
context (`${ctx:...}`, `${header:...}`, …) — never a whole hostname. The
template's fixed suffix is never produced by rendering; it is concatenated
onto the rendered label afterward. Because the suffix is a compile-time
constant, every possible render lands inside `pattern` by construction —
there is no rendered hostname to re-parse and no way for an injected value to
relocate the host.

The **dial target never moves**. The gateway still connects to the Service's
resolved endpoints exactly as it does today; the composed authority becomes
only the `Host` header (and, over TLS, the SNI and the certificate-validation
name). The composed authority is never resolved in DNS — this is what removes
the "dynamic forward proxy" confused-deputy risk class rather than merely
mitigating it. `upstreamAuthority` only engages for a pool Service: a
`ServiceClusterIp` or `ExternalName` target resolves to a fixed `Static`
target with no Service key to attach a policy to, so it never gets the
authority treatment.

**Fail-closed behavior.** A label that fails to render (the common case: a
`${ctx:...}` variable no plugin set) or renders to something that fails DNS-
label validation is a hard failure — **never** a silent fallback to the
static SNI/Host, which would send the request to the wrong virtual host.

- **Standard path**: `502`, with an `EdgionStatus` marking the authority
  rejection; the peer is left untouched.
- **Outbound path**: routed through the existing `allowDegradation` /
  `statusOnError` semantics on `ExternalHttpConnection` — no new failure
  vocabulary. This principle is now stated globally in the
  [unification-points registry](../../01-architecture/00-common/05-unification-points.md)
  (entry 2), and the vocabulary itself is slated for convergence in
  `tasks/todo/failure-policy-unification.md`.

**The `${ctx:...}` ordering constraint.** On the standard path the template
renders at peer-build time (`upstream_peer`). A plugin that writes
`ctx:tenant` must therefore run in `request_filter` or an earlier stage —
strictly **before** `upstream_peer`. A ctx variable written by a plugin stage
that runs later does not exist yet when the peer is built: the render yields
an empty label and the request fails closed (`502`, reason `render_empty` on
the `edgion_gateway_upstream_authority_rejected_total` metric). This is the
most likely operator mistake — watch that metric.

**Two residual risks, documented rather than mitigated in-process:**

- **The backend must not treat `Host` as an authorization credential.**
  Within the allowlist, a client picks which vhost on that backend it
  reaches — that is the backend's own authorization boundary, not the
  gateway's egress boundary. A backend that trusts `Host` for tenant identity
  is broken by this feature, regardless of how the gateway enforces the
  pattern.
- **Connection-pool / TLS-session cardinality scales with the number of live
  distinct authorities, not with the number of backends.** `reqwest` pools
  per `(scheme, host, port)` and Pingora's upstream reuse keys on the peer
  (which includes SNI), so N tenants sharing one destination address can open
  up to N distinct pools/TLS sessions. Cap idle connections per host if
  tenant cardinality is high. Cleartext backends do not pay this cost — with
  no SNI, only the `Host` header varies and one pool serves every tenant.

**Two known limitations:**

- **The Pingora active-health probe is cleartext-only today.**
  `healthCheckHost` fixes the `Host` header the standard-path probe sends,
  but the probe client carries no TLS configuration at all, so it does not
  set SNI or validate a certificate on that path. A TLS backend that
  dispatches on SNI (not just `Host`) is not correctly health-checkable via
  the standard path yet. The **outbound** path's own probe
  (`ExternalHttpConnection.healthCheck.active`) is unaffected — it does
  support TLS.
- **A mixed (partially-literal) label slot can defeat the fail-closed
  `render_empty` signal.** The empty-render check runs on the whole rendered
  label, not per variable. `${ctx:tenant}` alone with an unset `ctx:tenant`
  renders `""` and correctly fails closed. But a slot like `t${ctx:id}` with
  an unset `ctx:id` renders `"t"` — non-empty, and `"t"` is a perfectly valid
  DNS label — so the request silently routes to whatever vhost `t.<suffix>`
  happens to be, instead of failing closed. (`tenant-${ctx:id}` is *not* an
  example of this: its empty case renders `"tenant-"`, which fails the
  trailing-hyphen rule and is correctly rejected as `invalid_label` — the
  danger is specifically a mixed slot whose literal-only remainder still
  happens to pass DNS-label validation.) **Recommendation: use a label slot
  that is a single `${...}` variable with no literal characters**, so the
  `render_empty` fail-closed signal is reliable.

**Minimal example.**

```yaml
apiVersion: v1
kind: Service
metadata:
  name: sandbox-gw
  namespace: prod
spec:
  selector:
    app: sandbox-gw
  ports:
    - port: 443
      targetPort: 8443
---
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: sandbox-authority
  namespace: prod
spec:
  targetRefs:
    - kind: Service
      name: sandbox-gw
  upstreamAuthority:
    pattern: "*.sandbox.internal"
    template: "${ctx:tenant}.sandbox.internal"
    healthCheckHost: "probe.sandbox.internal"
---
# CtxSet runs in the RequestFilter stage, which is BEFORE upstream_peer, so
# ctx:tenant exists by the time the authority template renders.
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: sandbox-tenant-ctx
  namespace: prod
spec:
  requestPlugins:
    - type: CtxSet
      config:
        vars:
          - name: "tenant"
            from:
              type: header
              name: "X-Tenant-Id"
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: sandbox-route
  namespace: prod
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: sandbox-tenant-ctx
      backendRefs:
        - name: sandbox-gw
          port: 443
```

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
