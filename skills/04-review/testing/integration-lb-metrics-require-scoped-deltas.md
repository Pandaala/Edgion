---
name: integration-lb-metrics-require-scoped-deltas
description: Use when reviewing integration tests that infer load-balancer behavior from cumulative backend metrics.
---

# Integration LB metrics require scoped deltas

## Conclusion

An integration test that claims a load-balancing policy must fail unless every generated request succeeds
and the traffic produced by that case satisfies the policy assertion. Cumulative metric totals from earlier
cases or runs are not case evidence.

## Core rationale

Take a metric snapshot immediately before the requests and compare it with a snapshot afterward. Validate
the exact expected endpoint set and exact request delta before checking distribution. For strict RoundRobin,
send requests sequentially so scheduling order cannot obscure selector order; when the request count divides
the endpoint count, exact equal counts are the clearest deterministic contract.

When equivalent local and Kubernetes fixtures use different address forms, obtain the expected set from the
fixture authority: committed EndpointSlices locally and the Kubernetes API's Ready Service endpoints in a
cluster. Do not define the expected set from the Gateway metrics being tested. A one-cycle warmup can then
require exactly one successful request and one metric increment per authoritative endpoint before taking a
new baseline for the measured cycle.

If the assertion also verifies the selected Kubernetes Service, both local and Kubernetes integration
Gateway configs must enable `metrics.backend_requests.labels.backend`. Production keeps this potentially
high-cardinality resource label disabled by default; the integration-only override is what makes
`backend_namespace` and `backend_name` available to the test without weakening the production policy.

Strict sequential RoundRobin assertions assume one selector state. The current Kubernetes integration
topology deliberately runs one Gateway replica. If the test topology adds replicas, pin this case to one
Gateway instance or derive and document a bound that follows from the number of independent selector states.

Request transport errors, task join errors, and unexpected HTTP status codes are test failures. Values such
as `is_balanced` or variance only count as validation when they control the verdict, not when they appear only
in a success message.

## Fix suggestions not accepted

- Accepting any nonzero metric total, because stale traffic can satisfy it.
- Checking only the number of observed endpoints, because a pinned backend can coexist with stale series.
- Launching concurrent requests while claiming strict selection order.
- Adding a new product metric solely for the test; use the existing backend counter with a baseline delta.

## Re-evaluation triggers

Revisit exact equality if the supported test topology intentionally distributes one case across independent
Gateway selector states. In that topology, retain exact request and endpoint assertions and document the
smallest deterministic skew bound implied by the number of selector instances.

## Reference cases

- `edgion-tests/src/client/suites/http_route/backend/lb_roundrobin.rs`
