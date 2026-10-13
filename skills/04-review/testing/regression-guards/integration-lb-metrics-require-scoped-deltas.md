---
name: integration-lb-metrics-require-scoped-deltas
description: Use when reviewing integration tests that infer load-balancer behavior from cumulative backend metrics.
knowledge_type: regression-guards
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

## Retry-storm evidence is separate from distribution

ARCH-281 adds invocation-unique canonical W3C markers and stored access-log
inspection to the eight concurrent LeastConn retry-storm requests. A terminal
503 alone does not prove retries: each matching entry must contain exactly four
LeastConn attempts with status 503, `RetryOnStatus` on the first three only, and
well-formed reason arrays (the empty final array may be omitted by serialization).
Missing, stale, malformed or truncated evidence fails the case.

The suite drains its request tasks, bounds concurrent log queries to five seconds,
and then attempts deletion of only this invocation's eight stored entries under
a separate five-second bound. Query and cleanup failures affect the verdict.
The existing 30-second outer case timeout remains fail-closed; this does not
guarantee cleanup after outer cancellation or prove internal connection-ledger
accounting, which has separate `lb::attempt` unit tests.

Pure verdict tests cover valid chains, identity/count/shape failures, wrong
status/policy and misplaced retry markers. They do not establish a live retry
run. The existing healthy-phase cumulative-metric issue remains ARCH-280; a
verified storm does not repair or validate that separate distribution assertion.

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

## Resolution references

- `ITEST-011` — 2026-08-26.

## Retired orphan analyzer

ARCH-252 removes the unused file-based `AccessLogAnalyzer` module and its own
13 unit tests after a repository-wide caller/documentation search found no
consumer or supported offline-tool contract. Its permissive parsing and readable
trace-prefix selection were separate from the registered live suites.

`LBRoundRobinTestSuite` continues to own successful request and exact endpoint/
Service counter-delta assertions. Retry/request facts continue to use
`AccessLogClient` and the canonical marker-to-W3C-trace-ID helper. These existing
owners do not claim equivalence with every behavior of the retired offline API.
Live cases, registrations, fixtures, listeners and assertions are unchanged.

Verification compares the retained harness file set and contents and reruns the
five RoundRobin unit tests plus the marker identity test. This retirement fixes
no demonstrated live false-pass and makes no performance claim. Re-evaluate only
if an explicitly supported offline diagnostic consumer needs a defined identity,
parse-failure and insufficient-evidence contract.
