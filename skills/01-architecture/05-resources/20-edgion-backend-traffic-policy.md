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

| Target kind | Valid sections |
|-------------|----------------|
| `Service` | All existing sections |
| `EdgionBackend` | `connection`, `retryConstraint`, `circuitBreaker`, `outlierDetection` |

For AI targets, `loadBalancer`, `healthCheck`, and `upstreamAuthority` reject the whole policy.
Cross-service choice belongs to AiProxy; an AI service has one provider endpoint, and Edgion does
not invent an unauthenticated public-provider health-probe contract.

## Controller

- Validation calls the canonical target parser plus `spec.get_validation_errors()`.
- Parse clears old dependencies, registers every typed target, and resolves object existence per
  target.
- Service targets use `service_ref_mgr`; AI targets use `ai_backend_ref_mgr`.
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

`BackendTrafficPolicyStore` is the sole writer and publishes one ArcSwap snapshot keyed by typed
backend identity. It applies the same per-target oldest-wins arbitration and exposes stable handles
for connection timeout, retry budget, circuit breaker, outlier detection, and half-open state.

Service-only side effects—load-balancer selector compilation, endpoint active health checks,
dynamic authority, and panic threshold—remain isolated to Service keys. For AI targets, the
service-level circuit breaker is the aggregate concurrency guard; passive outlier state is keyed by
typed AI service plus credential alias so one failing credential does not eject its siblings.

`HTTPRoute.retry` authorizes a retry. EBTP `retryConstraint` only limits aggregate retry volume;
both must pass. No AI-specific retry policy or route-targeted EBTP is introduced.

## Key sources

- `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend_traffic_policy.rs`
- `edgion-gateway/src/backends/policy/backend_traffic/store.rs`
- `edgion-gateway/src/backends/policy/backend_traffic/conf_handler_impl.rs`
