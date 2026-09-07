---
name: resource-edgion-backend-traffic-policy
description: EdgionBackendTrafficPolicy typed Service and AI backend dependencies, conflict status, and Gateway arbitration.
---

# EdgionBackendTrafficPolicy architecture

EdgionBackendTrafficPolicy (EBTP) attaches to homogeneous same-Namespace `targetRefs` of either
core `Service` or `edgion.io/EdgionBackend`. It never targets HTTPRoute and never supplies
routing destinations. AiProxy selection still uses only `HTTPRoute.backendRefs`.

Typed identity is `(group, kind, namespace, name)`; Service and EdgionBackend objects with the
same Namespace/name remain distinct. The capability matrix is atomic:

| Target | Valid sections |
|--------|----------------|
| Core Service | All sections |
| AI EdgionBackend | `connection`, `retryConstraint`, `circuitBreaker`, `outlierDetection` |

AI targets reject `loadBalancer`, `healthCheck`, and `upstreamAuthority`. Provider selection is
route-scoped, provider health cannot be inferred through unauthenticated probes, and credential
outlier state is not a Service endpoint pool.

The Controller registers typed dependencies, validates target existence and capability, and
arbitrates conflicts by oldest creation timestamp then policy name. Apply/delete requeues only
affected policies through the EBTP workqueue.

The Gateway publishes one immutable ArcSwap snapshot keyed by typed target. AI requests capture the
complete effective-policy Arc for the selected backend at plan time. Core Service HTTP/gRPC
requests capture the same Arc once at first `begin_upstream_attempt` onto
`BackendContext.effective_policy` and reuse it for later attempts of that request. Health-check
and outlier **status** stay live; only configuration (including the outlier handle Arc) is
captured. HTTPRoute retry configuration authorizes retries, while `retryConstraint` only limits
aggregate retry volume.

The captured retry-budget Arc owns a monotonic origin and epoch-tagged atomic
counter buckets. Budget and minimum-rate windows share one elapsed sample per
operation but retain their independent intervals. See
[retry-budget epochs](../../12-algorithms/retry-budget-epoch-buckets.md) for
publication, saturation, delayed-operation, and lock-free platform boundaries.
