---
name: resource-edgion-backend-traffic-policy
description: EdgionBackendTrafficPolicy typed Service/EdgionBackend dependencies, conflict status, and Gateway arbitration.
---

# EdgionBackendTrafficPolicy resource

> Common flow: [00-resource-flow.md](00-resource-flow.md)
> External schema: [../../02-features/03-resources/20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md)

EdgionBackendTrafficPolicy (EBTP) is a direct backend policy. It attaches to homogeneous,
same-namespace `targetRefs` of either core `Service` or `edgion.io/EdgionBackend`. It never
targets HTTPRoute and never supplies forwarding destinations. Route-scoped AI selection belongs to
rule-level `AiProxy.backendSelection`; its candidate list, order, and weights come only from
`HTTPRoute.backendRefs`.

## Canonical target identity

Controller and Gateway share the resource-layer parser and typed identity:

```text
(group, kind, namespace, name)
```

Accepted pairs are core-group `Service` and `edgion.io/EdgionBackend`. A plain
`namespace/name` key is insufficient because both kinds may have the same name. Every target in one
policy must have the same kind. An EdgionBackend policy accepts 1–16 unique targets; Service
policies retain unbounded fan-out and treat duplicate references idempotently.

The field matrix is atomic:

| Resolved target capability | Valid sections |
|----------------------------|----------------|
| Core `Service` | All existing sections |
| AI `EdgionBackend` | `connection`, `retryConstraint`, `circuitBreaker`, `outlierDetection` |
| A2A Service `EdgionBackend` | `connection`, `retryConstraint`, `circuitBreaker`, `loadBalancer` without `panicThreshold`, and `upstreamAuthority` |
| A2A external `EdgionBackend` | `connection`, `retryConstraint`, `circuitBreaker` |

For AI targets, `loadBalancer`, `healthCheck`, and `upstreamAuthority` reject the whole policy. A2A
Service targets reject `healthCheck`, `outlierDetection`, and `loadBalancer.panicThreshold` because
WP4 has no canonical captured health population for that nested Service. A2A external targets also
reject load balancing and authority override. Cross-service AI choice belongs to AiProxy; Edgion
does not invent an unauthenticated public-provider health-probe contract.

## Controller

- Validation calls the canonical target parser plus `spec.get_validation_errors()`.
- Parse clears old dependencies, registers every typed target, and resolves object existence per
  target.
- Service targets use `service_ref_mgr`; all EdgionBackend variants use the shared typed backend
  dependency index. EdgionBackend capability is derived from the current RawStore object after
  full spec validation because backend `on_change` requeues policy workers before processed-cache
  publication. A valid A2A identity may retain an inert marker while its nested target is
  unresolved; missing `resolvedA2a` still withdraws the Gateway runtime.
- Valid and resolved targets enter conflict arbitration independently. An unresolved sibling is
  omitted while its dependency remains registered for self-healing.
- Policies sharing a typed target use oldest creation timestamp, then policy name, as precedence.
  A multi-target policy may win some targets and lose others.
- Apply/delete requeues only affected sibling policies through the EBTP workqueue. EBTP changes do
  not rebuild HTTPRoutes.

Status uses per-Gateway ancestors. `Accepted` reports schema, field-matrix validity, and
oldest-wins arbitration: a losing target is reported as `Accepted=False` with reason `Conflicted`
(GEP-2648; there is no separate `Conflicted` condition type). `ResolvedRefs` reports target
existence.

## Gateway

`BackendTrafficPolicyStore` is the sole writer and publishes one immutable ArcSwap snapshot keyed by
typed backend identity. The snapshot has a monotonic generation. Full and partial reconciles compute
all affected winners before one publication, so a reader cannot observe a half-applied target set.
Equivalent updates retain the outer effective-policy Arc and generation; captured old Arcs remain
valid for in-flight requests. It applies the same per-target oldest-wins arbitration and exposes
stable handles for connection timeout, retry budget, circuit breaker, outlier detection, and
half-open state.

AI and A2A capture one complete policy snapshot for their selected direct EdgionBackend. Core
Service deliberately retains its existing per-attempt live compatibility lookups. EdgionBackend
runtime, EBTP, and BackendTLSPolicy are independent conf_sync publications; there is no
cross-resource transaction.

Core-Service side effects—active health checks, passive endpoint outlier state, and panic
threshold—remain isolated to Service keys. A2A Service reuses the canonical selector cell with the
direct EdgionBackend's load-balancer algorithm and authority, but does not inherit nested Service
policy state. For AI and A2A, the direct logical circuit breaker is the aggregate concurrency
guard; AI passive outlier state remains keyed by typed service plus credential alias.

`HTTPRoute.retry` authorizes a retry. EBTP `retryConstraint` only limits aggregate retry volume;
both must pass. No AI-specific retry policy or route-targeted EBTP is introduced.

## Key sources

- `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend_traffic_policy.rs`
- `edgion-gateway/src/backends/policy/backend_traffic/store.rs`
- `edgion-gateway/src/backends/policy/backend_traffic/conf_handler_impl.rs`
