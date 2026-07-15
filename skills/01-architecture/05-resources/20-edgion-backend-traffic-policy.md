---
name: resource-edgion-backend-traffic-policy
description: EdgionBackendTrafficPolicy Controller processing, Service dependency/conflict tracking, status, and Gateway arbitration.
---

# EdgionBackendTrafficPolicy resource

> Common flow: [00-resource-flow.md](00-resource-flow.md)
> External schema: [../../02-features/03-resources/20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md)

The policy attaches load balancing, active health check, outlier detection, and panic-threshold
configuration to same-namespace Services through `targetRefs`.

## Controller

- `validate` calls `spec.get_validation_errors()`.
- `parse` registers every Service target in `runtime.service_ref_mgr` so Service appearance/removal
  requeues the policy.
- Valid policies also register in the runtime conflict index. Invalid policies remain no-ops and do
  not participate in precedence.
- Policies that target the same Service use oldest creation timestamp, then name, as precedence.
  A target/timestamp/validity change requeues affected siblings without unconditional ping-pong.
- `on_delete` clears Service/conflict registrations and requeues surviving siblings.

Status conditions:

| Condition | Meaning |
|-----------|---------|
| `Accepted` | Spec validation result |
| `ResolvedRefs` | All target Services currently exist |
| `Conflicted` | A valid older sibling wins at least one shared Service target |

## Gateway

`BackendTrafficPolicyStore` caches policies and recomputes only Service keys affected by a full or
partial update. It skips invalid specs, applies the same oldest-wins arbitration, and is the sole
writer of effective per-Service:

- active health-check configuration;
- circuit-breaker/outlier configuration;
- panic-threshold override;
- parsed LB policy.

The request path resolves the effective algorithm from this store; selector cells build an
algorithm-specific compiled selector lazily.

## Key sources

- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend_traffic_policy.rs`
- `edgion-gateway/src/backends/policy/backend_traffic/store.rs`
- `edgion-gateway/src/backends/policy/backend_traffic/conf_handler_impl.rs`
- `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`
