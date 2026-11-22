---
name: ready-endpoints-during-service-drain
description: Selection of EndpointSlice backends for new requests during a Service rollout.
knowledge_type: intentional-design
---

# Ready endpoints during Service drain

For infra3-01, the project accepted HTTP 503 when a Service port has zero
ready endpoints during a rollout. Restricting ordinary Service load balancing
to ready endpoints withdraws a backend from new traffic when every source for
its address and port explicitly reports `ready: false`. This decision concerns new selections, including sticky
lookups. It does not govern the lifetime of an established connection.

In Gateway discovery, `materialize_section` builds all four selectors from
`ready_slots`. Health degradation relaxes health filtering within that set.
`CapturedServiceSelection::endpoint_for_addr` also checks slot readiness.
See the [readiness regression guard](../regression-guards/endpoint-slice-ready-condition-semantics.md)
for absent conditions and addresses reported by multiple slices.

When the last ready endpoint starts terminating before its replacement is
ready, new HTTP requests can receive 503 even while the old Pod can serve.
Kubernetes [permits routing to serving, terminating endpoints](https://kubernetes.io/docs/concepts/services-networking/endpoint-slices/#terminating)
in this window. Revisit that fallback if the product requires new requests to
reach draining Pods during a zero-ready rollout window.
