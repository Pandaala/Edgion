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

Ordinary HTTP and GRPC routes use one public retry contract: `attempts` is the
number of retries after the initial upstream request, so total upstream requests
are bounded by `1 + attempts`. A route's `rules[].retry.attempts` overrides
`EdgionGatewayConfig.spec.retry.attempts`, whose default is 2. The shared
representable domain is `0..=i32::MAX`, matching the Gateway API route field.

Pingora's `ServerConf.max_retries` counts loop iterations including the initial
request. Edgion therefore configures it to `i32::MAX + 1`; it is capacity for
the public route budget, not a second operator-visible retry policy.

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
Pingora loop capacity. Re-evaluate if Gateway API changes the type or bounds of
`retry.attempts`, Pingora changes `max_retries` semantics, or retry counting no
longer increments before upstream selection. Also re-evaluate D0010 if AI's
backend/dispatch bounds increase to approach the Pingora capacity or endpoint
connect failures stop advancing at backend scope.

## Resolution references

- D0009, fixed 2026-09-21; original task: “Align Route Retry Limits with
  the Pingora Loop Ceiling.”
- D0010, resolved by removing the independent Pingora 16-iteration terminal
  boundary; the AI retry budget and renderer remain independently owned.
