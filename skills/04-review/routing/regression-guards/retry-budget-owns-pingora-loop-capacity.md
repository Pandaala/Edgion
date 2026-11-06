---
name: retry-budget-owns-pingora-loop-capacity
description: Use when reviewing ordinary HTTP or GRPC retry counts, EdgionGatewayConfig retry defaults, or Pingora ServerConf max_retries.
status: fixed
finding_id: D0009
closed: 2026-09-21
knowledge_type: regression-guards
---

# Route Retry Budget Owns the Pingora Loop Capacity

## Conclusion and scope

Ordinary policy-driven HTTP and GRPC retries use one public contract: `attempts` is the
number of retries after the initial upstream request, so total upstream requests
are bounded by `1 + attempts`. A route's `rules[].retry.attempts` overrides
`EdgionGatewayConfig.spec.retry.attempts`, whose default is 2. The global
resource accepts `0..=i32::MAX`; the current Gateway API route retry field
requires `1..=i32::MAX`. Use the global zero default to disable policy-driven
retries; an explicit `rules[].retry.attempts: 0` is rejected at ingest.
P090-008 revalidated this distinction against `HTTPRouteRetry::validation_error`
and the vendored v1.6.2 Experimental schema.

Pingora's `ServerConf.max_retries` counts loop iterations including the initial
request. Edgion therefore configures it to `i32::MAX + 1`; it is capacity for
the public route budget, not a second operator-visible retry policy.

Raising that capacity also removed the only bound on retries Pingora decides by
itself. At the locked fork `edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`
(`h2` 0.4.20), `handle_read_header_error` marks peer-qualified
h2 GOAWAY, RST_STREAM(REFUSED_STREAM), and local protocol/downgrade outcomes as
`RetryType::Decided(true)` before `error_while_proxy` ever sees them, and
`RetryType::decide_reuse` is a no-op once a decision exists, so the route
retry budget cannot veto them through `decide_reuse` alone. Real IO errors now
arrive as `ReusedOnly`, not `Decided(true)`, and pass the ordinary reuse gates.
The explicit ambiguous-H2 guard and AI dispatcher can revoke pre-decided
outcomes; see [the method-safety policy table](retry-non-idempotent-method-guard.md).
`error_while_proxy`
therefore counts those pre-decided failures in `EdgionHttpContext::pingora_decided_retries`
and permits at most `MAX_PINGORA_DECIDED_RETRIES` (16) retries. An always-refused
origin therefore sees exactly 17 requests including the initial attempt,
then `UpstreamTransportRetryCeiling`. This is a retry count, distinct from the
former 16-iteration Pingora loop. Retries this callback sets itself are not
counted; they already passed their own gates.

These fork claims (`handle_read_header_error` classification in
`pingora-core/src/protocols/http/v2/client.rs`, `RetryType::decide_reuse` in
`pingora-error/src/lib.rs`, and the `while retries < self.max_retries` loop in
`pingora-proxy/src/lib.rs`) were re-verified by source inspection at that
revision on 2026-11-06 after the fork was rebased onto upstream 0.9.1; the first
two files are unchanged from `0b0d8bba` (the `edgion_0.9.0_v1` revision this page was last verified against; not the immediate predecessor lock, which was `1aaa45cc`). Historical: the
original observation was made at fork `0b0d8bb` on the `edgion_0.9.0_v1` branch.

## Rationale and evidence

The locked Pingora loop increments its counter once per upstream attempt and
continues while that counter is below `ServerConf.max_retries`. Leaving
Pingora's default of 16 made accepted route values above 15 unexecutable.

The resource schema now validates the global default before publication, the
CRD carries the same maximum, and the gateway defensively falls back to the
default before publishing a request snapshot if invalid conf-sync input bypasses
controller validation. Request callbacks only compare the completed-attempt
counter with the already-resolved retry budget.

## Alternatives and limits

A product maximum of 15 was rejected because it exposes an implementation
default as policy and is substantially narrower than the Gateway API field.
Silently clamping individual request callbacks was rejected because accepted
configuration would still disagree with execution. AI remains independently
bounded to 16 candidate backends and 8 dispatched attempts. Both bounds are
below Pingora's new capacity, so AI exhaustion returns through its own terminal
renderer instead of ending at an unrelated 16-iteration ceiling.

## Verification and re-evaluation

Regression coverage checks default 2, explicit 0, `i32::MAX`, rejection of one
above the maximum in the global resource, the `1 + attempts` decision boundary,
gateway publication fallback, runtime hot reload, CRD parity, and the explicit
Pingora loop capacity. The Pingora-decided ceiling is covered by the
`pingora_decided_retries_are_bounded_by_the_fail_safe_ceiling` unit test and
the P090-008 `HTTPRoute/Transport` `refused_ceiling` cases assert exact
request/effect counts and the terminal diagnostic. The older
`HTTPRoute/Protocol/H2TrailerMalformed` suite checks error/liveness behavior
against an always-refused origin but does not establish the exact ceiling.

Fresh P090-008 verification on 2026-10-05 passed the route-budget unit tests
(3/3), override unit tests (11/11), and the full Transport suite (145/145,
including 12 retained explicit unsupported-shape skips). Both new
`refused_ceiling` cases executed and proved exactly 17 origin requests, zero
application effects, and `UpstreamTransportRetryCeiling`. The reviewed
[policy and acceptance record](retry-non-idempotent-method-guard.md#acceptance-evidence-and-limits)
distinguishes controlled-origin results from source/unit evidence and records
the local integration environment limits.

Re-evaluate if Gateway API changes the type or bounds of `retry.attempts`, Pingora changes `max_retries` semantics, or retry counting no
longer increments before upstream selection. Also re-evaluate D0010 if AI's
backend/dispatch bounds increase to approach the Pingora capacity or endpoint
connect failures stop advancing at backend scope.

## Resolution references

- D0009, fixed 2026-09-21; original task: “Align Route Retry Limits with
  the Pingora Loop Ceiling.”
- D0010, resolved by removing the independent Pingora 16-iteration terminal
  boundary; the AI retry budget and renderer remain independently owned.
- Follow-up fixed 2026-09-22: the removed 16-iteration boundary had also been
  the only ceiling on Pingora-decided transport retries, so an upstream that
  always answered RST_STREAM(REFUSED_STREAM) drove ~42k upstream attempts per
  request until the downstream client timed out. The ceiling is now owned
  explicitly by `pg_error_while_proxy`.
