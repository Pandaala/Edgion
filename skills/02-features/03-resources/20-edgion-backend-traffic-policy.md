---
name: edgion-backend-traffic-policy-features
description: Canonical EdgionBackendTrafficPolicy schema and variant-aware capability matrix for direct Service, AI, and A2A targets.
---

# EdgionBackendTrafficPolicy resource

> API: `edgion.io/v1` | Scope: Namespaced | Short name: `ebtp`
> Internal processing: [../../01-architecture/05-resources/20-edgion-backend-traffic-policy.md](../../01-architecture/05-resources/20-edgion-backend-traffic-policy.md)

EdgionBackendTrafficPolicy (EBTP) configures one or more direct backend destinations. A policy
targets either core Services or EdgionBackends in its own Namespace. It does not target HTTPRoute
and does not define a backend list for routing.

For AI traffic, HTTPRoute `backendRefs` remains the only candidate, order, and weight source;
rule-level `AiProxy.backendSelection` chooses among those candidates. For A2A, the direct
EdgionBackend remains the policy identity even when it resolves to a nested Service. EBTP never
attaches through that nested Service and never changes route weights.

## Target contract

```yaml
targetRefs:
  - group: ""
    kind: Service
    name: api-service
```

or:

```yaml
targetRefs:
  - group: edgion.io
    kind: EdgionBackend
    name: primary-provider
```

Rules:

- A policy has at least one `targetRef`
- An EdgionBackend policy has at most 16 `targetRefs`; Service policies have no fixed maximum
- Every target is in the policy's Namespace; cross-Namespace targets are not supported
- Accepted target pairs are core-group `Service` and `edgion.io/EdgionBackend`
- All refs in one policy have the same target kind
- Duplicate EdgionBackend targets are rejected; duplicate Service targets are idempotent
- A Service and EdgionBackend with the same Namespace/name are distinct targets
- EdgionBackend refs in one policy must resolve to one homogeneous capability: AI, A2A Service, or
  A2A external
- HTTPRoute, GRPCRoute, and other route kinds are not valid targets

## Section applicability

| Section | Core Service | AI EdgionBackend | A2A Service EdgionBackend | A2A external EdgionBackend |
|---------|--------------|------------------|---------------------------|----------------------------|
| `loadBalancer.type` and `consistentHash` | Supported | Rejected | Supported | Rejected |
| `loadBalancer.panicThreshold` | Supported | Rejected | Rejected | Rejected |
| `healthCheck` | Supported | Rejected | Rejected | Rejected |
| `outlierDetection` | Supported | Supported | Rejected | Rejected |
| `retryConstraint` | Supported | Supported | Supported | Supported |
| `circuitBreaker` | Supported | Supported | Supported | Supported |
| `upstreamAuthority` | Supported | Rejected | Supported | Rejected |
| `connection` | Supported | Supported | Supported | Supported |

Unsupported sections reject the entire policy; they are never silently ignored. A2A Service
targets reuse the canonical Service selector for EndpointSlice discovery, but WP4 has no captured
health/outlier population and therefore rejects active health checks, passive outlier detection,
and panic-threshold fail-open. A2A external targets have one endpoint and reject load balancing and
authority overrides. AI retains its credential-scoped passive outlier behavior.

Policy attachment follows an existing, fully valid direct EdgionBackend identity. If an A2A
Service target later becomes missing or cross-Namespace authorization is denied, the policy may
remain accepted but is inert: the backend publishes no `resolvedA2a`, the Gateway withdraws its
runtime entry, and traffic fails closed until eligibility recovers.

## Complete Service schema example

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
    panicThreshold: 30
  healthCheck:
    active:
      type: http
      path: /healthz
      port: 8080
      interval: 10s
      timeout: 3s
      healthyThreshold: 2
      unhealthyThreshold: 3
      expectedStatuses: [200]
  outlierDetection:
    consecutiveErrors: 5
    consecutiveGatewayErrors: 5
    consecutiveLocalOriginFailures: 5
    ejectionTime: 30s
    maxEjectionTime: 5m
    maxEjectionPercent: 50
  retryConstraint:
    budget:
      percent: 20
      interval: 10s
    minRetryRate:
      count: 10
      interval: 1s
  circuitBreaker:
    maxParallelRequests: 200
  connection:
    connectTimeout: 300ms
```

Every policy section is optional. An absent section means the normal global or engine default for
that concern. One winning policy contributes its complete configuration atomically; Edgion never
merges fields from competing policies.

## Configuration fields

### `targetRefs`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `group` | String | No | Core API group | `""` or `core` for Service; exactly `edgion.io` for EdgionBackend |
| `kind` | String | Yes | — | `Service` or `EdgionBackend`, paired with the correct group |
| `name` | String | Yes | — | Same-Namespace target name |

### `loadBalancer` (core Service and A2A Service)

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `type` | Enum | Yes with section | — | `RoundRobin`, `LeastConn`, `Ewma`, or `ConsistentHash` |
| `consistentHash` | Object | With `ConsistentHash` | — | Hash source and optional key |
| `consistentHash.hashOn` | Enum | With `ConsistentHash` | — | `header`, `cookie`, `queryParam`, or `sourceIp` |
| `consistentHash.key` | String | Conditional | — | Required for header/cookie/query; absent for `sourceIp` |
| `panicThreshold` | Integer | No | Global value | Percentage from 0 through 100; core Service only, rejected for A2A Service |

The field is named `hashOn`, not `on`, because YAML 1.1 converters may interpret an unquoted `on`
key as a Boolean.

### `healthCheck` (core Service only)

`healthCheck.active` uses the shared active-health-check shape. Supported probe types are `http`,
`tcp`, and `grpc`. Thresholds are at least one; interval and timeout are non-zero; HTTP requires a
non-empty path and status list. See [Service backends](16-service-backends.md) for probe behavior.

### `outlierDetection`

| Field | Meaning |
|-------|---------|
| `consecutiveErrors` | Application 5xx threshold |
| `consecutiveGatewayErrors` | Upstream 502/503/504 threshold |
| `consecutiveLocalOriginFailures` | Connect/TLS/transport threshold |
| `ejectionTime` | Initial ejection duration |
| `maxEjectionTime` | Maximum bounded ejection duration |
| `maxEjectionPercent` | Maximum ejected share for a Service endpoint pool |

Classification is per upstream attempt. For core Service targets, state is per endpoint. For AI
EdgionBackend targets, the stable destination is the typed service and passive state is further
isolated by credential alias, so one failing or rate-limited credential does not eject its
siblings. A half-open lease admits one probe after ejection expires. A2A Service and external
targets reject this section in WP4.

### `retryConstraint`

Retry constraint is an aggregate anti-storm budget, not permission to retry. `HTTPRoute.retry`
decides whether a request may retry, and EBTP decides whether aggregate retry volume remains within
budget. Both must pass.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `budget.percent` | Integer | No | `20` | Retry share of total attempts, 0–100 |
| `budget.interval` | Duration | No | `10s` | Sliding window from 1 s through 1 h |
| `minRetryRate.count` | Integer | No | `10` | Low-traffic floor, 1–1,000,000 retries |
| `minRetryRate.interval` | Duration | No | `1s` | Independent floor window, greater than zero and at most 1 h |

`retryConstraint: {}` enables the defaults. Absence disables the aggregate gate. A retry is
admitted when it is under either the percentage ceiling or the floor. The percentage denominator
counts all dispatched attempts. For AI traffic, pre-dispatch skips consume neither route retry
capacity nor retry-budget counters; a retryable dispatched failure charges the failed source
EdgionBackend.

### `circuitBreaker`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `maxParallelRequests` | Positive integer | Yes with section | — | Core Service: maximum concurrent requests per endpoint. AI or A2A EdgionBackend: aggregate concurrent requests for the direct logical backend |

Excess requests fail without queueing. For AI traffic, credential-specific outlier state remains
separate from this aggregate service guard.

### `upstreamAuthority` (core Service and A2A Service)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `pattern` | String | Yes | One leading wildcard label, such as `*.sandbox.internal` |
| `template` | String | Yes | Produces only the wildcard label; fixed suffix must match `pattern` |
| `healthCheckHost` | String | With active health check | Static authority for request-less probes |

The dial target remains the Service endpoints. The rendered authority becomes only HTTP Host and,
for TLS, SNI and certificate-validation name. Rendering failure or an invalid DNS label fails
closed; it never falls back to the static authority. Prefer a label made of one `${...}` variable,
without literal characters, so an unset variable reliably renders empty.

For A2A Service, this is a policy on the direct EdgionBackend, not on the nested Service. The same
rendered Host/SNI behavior applies. `loadBalancer.panicThreshold`, health-check-only
`healthCheckHost`, and active probes remain unavailable for this target class.

### `connection`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `connectTimeout` | GEP-2257 duration | No | Global HTTP backend connect timeout | Per-target connection-establishment deadline, greater than zero and at most 1 h |

This field applies to HTTP upstream connections, including AI and A2A connections. It does not
change TCPRoute/TLSRoute stream connect deadlines. Request and backend-request deadlines remain on
HTTPRoute; EdgionBackend has no timeout field.

## EdgionBackend example

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: ai-provider-resilience
  namespace: production
spec:
  targetRefs:
    - group: edgion.io
      kind: EdgionBackend
      name: primary-provider
  connection:
    connectTimeout: 2s
  retryConstraint:
    budget:
      percent: 20
      interval: 10s
  circuitBreaker:
    maxParallelRequests: 200
  outlierDetection:
    consecutiveErrors: 5
    consecutiveGatewayErrors: 3
    consecutiveLocalOriginFailures: 3
    ejectionTime: 30s
    maxEjectionTime: 5m
```

The HTTPRoute continues to reference `primary-provider` directly. This policy neither attaches to
the route nor adds the provider to any route.

## A2A policy and BackendTLSPolicy semantics

For an A2A Service target, EBTP may configure load-balancer type/consistent hash,
`upstreamAuthority`, retry constraint, aggregate circuit breaker, and connection timeout. For an
A2A external target, only retry constraint, aggregate circuit breaker, and connection timeout are
accepted. These sections are captured from the policy on the direct EdgionBackend; a policy on the
nested Service is not composed into the request.

BackendTLSPolicy also targets the direct `edgion.io/EdgionBackend`. It uses the canonical upstream
TLS resolver and peer builder for CA, mTLS, hostname validation, SNI, and authority behavior. A
BackendTLSPolicy on a cleartext A2A external endpoint fails closed. When an A2A Service policy uses
`upstreamAuthority`, its wildcard pattern must contain every matching BackendTLSPolicy validation
hostname; otherwise the EBTP is rejected. Retry permission still comes from HTTPRoute, and the
existing session-persistence and accounting semantics are unchanged.

## MCP policy and BackendTLSPolicy semantics

Policy always targets the directly routed `edgion.io/EdgionBackend`; a wrapped Service supplies
EndpointSlice discovery only and contributes no inherited policy.

| EBTP field | MCP Service | MCP external |
|------------|-------------|--------------|
| `loadBalancer.type`, `consistentHash` | Supported | Rejected |
| `loadBalancer.panicThreshold` | Rejected | Rejected |
| `healthCheck` | Rejected | Rejected |
| `outlierDetection` | Rejected | Rejected |
| `upstreamAuthority` | Supported | Rejected |
| `retryConstraint` | Rejected | Rejected |
| `circuitBreaker` | Supported | Supported |
| `connection.connectTimeout` | Supported | Supported |

Every present rejected field rejects the complete policy; the Gateway repeats capability checks
before atomic publication. Active health checks have no MCP-safe probe. Retry remains rejected
until a later frontend can prove safe no-write/replay semantics; a budget alone is not permission
to replay. Session persistence is not an EBTP field and is inactive for MCP in WP9.

BackendTLSPolicy supports its complete validation, CA, SAN, system-CA, and client-certificate
contract for MCP Service and HTTPS external targets. It rejects EdgionBackend `sectionName`, unknown
options, and every attachment to a cleartext external target. The Gateway checks the current MCP
runtime shape so stale or forged `McpExternal` markers cannot attach TLS to HTTP.

## Conflict resolution and status

Arbitration runs independently for each typed target. Oldest `creationTimestamp` wins; missing
timestamps lose to timestamped policies, and the policy name breaks remaining ties. A multi-target
policy may win one target and lose another. Deleting or invalidating the winner promotes the next
valid resolved sibling.

`status.ancestors` follows GEP-2648 and contains at most 16 Gateway ancestor entries. Each entry
contains `Accepted` and `ResolvedRefs` conditions:

| Condition/reason | Meaning |
|------------------|---------|
| `Accepted=True/Accepted` | Spec is valid and wins every relevant target for this ancestor |
| `Accepted=False/Invalid` | Structural, field-matrix, or section validation failed |
| `Accepted=False/Conflicted` | An older policy wins a relevant typed target |
| `ResolvedRefs=True/ResolvedRefs` | Relevant target objects exist |
| `ResolvedRefs=False` | One or more relevant Service or EdgionBackend objects do not exist |

An unreferenced policy has an empty ancestor list. Target apply/delete and route dependency changes
requeue the policy through its serialized workqueue so status and arbitration self-heal.

## Defaults without a matching section

| Concern | Default |
|---------|---------|
| Load balancing | Core Service and A2A Service use `RoundRobin`; AI choice remains AiProxy's route-scoped selection; A2A external has one endpoint |
| Active health check | None |
| Outlier detection | None |
| Retry constraint | None; only configured HTTPRoute retry permission applies |
| Circuit breaker | No concurrency cap |
| Connect timeout | Global HTTP backend connect timeout |
| Panic threshold | Global load-balancing value for core Service targets; unsupported for A2A Service |

## Related documentation

- [Service backends](16-service-backends.md)
- [EdgionBackend](22-edgion-backend.md)
- [BackendTLSPolicy](12-backend-tls-policy.md)
- [Internal backend-policy processing](../../01-architecture/05-resources/20-edgion-backend-traffic-policy.md)
