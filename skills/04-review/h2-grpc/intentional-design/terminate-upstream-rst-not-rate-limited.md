---
name: terminate-upstream-rst-not-rate-limited
description: Use when reviewing findings that terminate-driven upstream HEADERS→RST_STREAM cycles are not rate-limited (Rapid Reset shape aimed at the origin); covers the accepted tradeoff and its concrete re-evaluation trigger.
status: accepted-tradeoff
finding_id: transport-gap-08
closed: 2026-08-04
knowledge_type: intentional-design
---

# Terminate-driven upstream RST_STREAM is not rate-limited — ACCEPTED-RISK

## Conclusion

A client that reliably trips a mid-body policy drives one HEADERS→RST_STREAM
cycle per request against the upstream — the Rapid Reset shape
(CVE-2023-44487), aimed at the origin rather than at Edgion. Because an H2
upstream connection is shared across tenants, an origin enforcing a post-CVE
RST budget could GOAWAY that connection for everyone on it. This was judged an
application-layer concern during the request-body seam work and the fork was
deliberately not changed. The tradeoff is accepted **because no
terminate-heavy body policy ships today**, and the correct mitigation lives in
Edgion's plugin ordering, not in the proxy.

Fix suggestions of the form "add an RST_STREAM budget/rate limiter to the
fork's upstream H2 path" are **not accepted** while the trigger below has not
fired. Writing a test that asserts "N terminates produce N upstream resets" is
also **not accepted** — it would pin the exact shape we may want to eliminate.

## Core Rationale

**1. The mitigation point is before the upstream stream opens**

The risk exists only when a policy decides to terminate *after* the upstream
stream is already open (mid-body). If the deciding plugin runs in the request
stage — before `upstream_peer` / stream open — the reject produces no upstream
frame at all and the risk disappears with zero proxy change. That ordering
decision belongs to Edgion plugin architecture, not the fork.

**2. No shipping trigger**

Today's body policies reject either pre-upstream (request-stage gates over the
shared `BodyStorage`) or so rarely mid-body that the reset volume is noise. A
budget in the proxy would be speculative machinery with no consumer, against
the observability-restraint and reuse-first rules.

**3. A test today would freeze the deficiency**

Asserting the current one-reset-per-terminate behavior converts a thing we may
fix into a thing that fails CI when someone fixes it (the Class 4 rule:
decision first, then change, then test).

## Fix Suggestions Not Accepted

- Fork-side upstream RST_STREAM budget/rate limiter — no shipping
  terminate-heavy policy; the plugin-ordering mitigation is strictly better
  when applicable.
- Integration test pinning "N terminates → N upstream resets" — freezes the
  undesirable shape as contract.

## Re-evaluation Triggers

Re-open this decision only if:

- A terminate-heavy body policy actually ships (a plugin or ExtProc mode that
  routinely rejects mid-body at per-request rates). Then decide: (a) move the
  decision into the request stage so the reject happens before the upstream
  stream opens — preferred, zero proxy change; or (b) if that ordering is
  impossible for the policy, promote a fork task to add an upstream RST
  budget.
- An origin-side GOAWAY incident is traced to Edgion-originated reset volume.

## Reference Cases

- `tasks/todo/transport-coverage-observability-gaps.md` gap 8 (origin of the
  finding).
- `tasks/todo/important_test/04-fix-the-product-before-writing-the-test.md`
  Item B (the decide-first rule; closed 2026-08-04 with this entry).
- Background: CVE-2023-44487 (HTTP/2 Rapid Reset).

## Resolution references

- `transport-gap-08` — 2026-08-04 (⚠️ accepted tradeoff).
