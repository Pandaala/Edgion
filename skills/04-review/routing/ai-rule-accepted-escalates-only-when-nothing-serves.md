---
name: ai-rule-accepted-escalates-only-when-nothing-serves
description: Use when reviewing HTTPRoute status-condition findings about AI attachment/retry-cap violations; covers when a per-rule AI finding may raise route-level Accepted=False versus PartiallyInvalid.
status: fixed
finding_id: ctrl-01
closed: 2026-08-04
---

# AI rule invalidity escalates to `Accepted=False` only when no rule serves — FIXED

## Conclusion

An AI attachment or retry-cap violation is a **per-rule** finding. It always drops the offending
rule from the data plane, but it raises the route-level `Accepted=False/UnsupportedValue` **only
when no rule of the route serves any more**; while any sibling rule survives it rides
`PartiallyInvalid=True/RuleInvalid` with `(N of M rules active)` and leaves `Accepted` alone. When
the escalation does fire, `PartiallyInvalid` is never set `True` — an earlier `True` is cleared to
`False` — so the two conditions can never contradict each other.

Fix suggestions of the form "delete the `Accepted` push entirely — per-rule problems never flip
`Accepted`" or "keep the unconditional `Accepted` push and just stop feeding AI reasons into
`PartiallyInvalid`" are **not accepted**.

## Core Rationale

**1. `Accepted=False` is not informational — it removes the whole route**

`edgion-gateway/src/routes/common.rs` `is_parent_ref_accepted` / `filter_accepted_parent_refs`
exclude any parentRef whose status is not `Accepted=True`, and a route with no surviving parentRef
is not compiled at all. The pre-fix code pushed every AI violation into `validation_accepted_errors`
unconditionally, so one mistyped `retry.attempts` took an entire HTTPRoute — including unrelated,
perfectly valid non-AI rules — out of service.

**2. The same reason was emitted on two contradictory paths**

`ai_rule_invalid_reason` fed both `validation_accepted_errors` (→ `Accepted=False`) and
`find_controller_invalid_rule_details` (→ `invalid_rule_indices` + `PartiallyInvalid` with
`(N of M rules active)`). A mixed route therefore advertised "route not accepted" and "3 of 4 rules
active" on the same parent status, while the gateway actually served zero rules. Whichever contract
wins, emitting both is a defect.

**3. The escalation preserves the AI-3 intent without breaking the HTTPRoute precedent**

AI-3 (`01-design.md` §AiProxy attachment, `06-decisions.md` #8) deliberately chose
`Accepted=False/UnsupportedValue` so an invalid AI route does not reach the runtime. Gating that
push on "nothing serves" keeps that outcome for the common case — a route dedicated to AI whose only
rule is invalid — while restoring the documented per-rule precedent (`http_route.rs` Accepted note,
`skills/02-features/03-resources/04-httproute.md`, the CORS/rewrite tests) for mixed routes. The two
branches are naturally exclusive: `PartiallyInvalid`'s `own_invalid` already requires
`valid_rules > 0`.

**4. Delegation drops still never flip `Accepted`**

An AI violation inside a *delegated child* rule is removed from `resolved_rules` during parse and
reported as `DelegationIssue::RuleDropped`. That path keeps the pre-existing contract — a dropped
delegated rule never raises `Accepted` — and the fix does not change it. Only the route's own
`spec.rules` participate in the escalation.

## Implementation

`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`,
`update_status`:

- AI reasons are collected into `ai_invalid_rules` but no longer pushed eagerly.
- `serves_no_rule` = `resolved_rules.is_empty()` when delegation produced a resolved set, otherwise
  `valid_rules == 0`.
- `ai_rejects_route = !ai_invalid_rules.is_empty() && serves_no_rule` gates the
  `AcceptedError::UnsupportedValue` push, and also suppresses the `PartiallyInvalid=True` branch.
- When the escalation fires on a later reconcile (an operator deletes the last valid sibling rule),
  a previously-set `PartiallyInvalid=True` is cleared to `False` with reason `UnsupportedValue` and
  "Route not accepted; no rule is active" instead of the generic "All rules are valid", which would
  contradict the `Accepted=False` beside it.

Regression tests: `ai_invalid_rule_stays_partially_invalid_when_other_rules_survive` and
`ai_invalid_rule_rejects_route_only_when_nothing_serves` (same module).

## Fix Suggestions Not Accepted

- Remove the `Accepted` push outright (align purely with the HTTPRoute per-rule precedent) — it
  contradicts AI-3's explicit decision and lets a wholly invalid AI route sit at `Accepted=True`
  with an empty rule set, which reads as healthy.
- Keep the unconditional `Accepted` push and only stop feeding AI reasons into `PartiallyInvalid` —
  preserves the real harm (valid sibling rules taken out of service by one bad AI rule).
- Suppress `PartiallyInvalid` whenever `Accepted=False` for any reason — `Accepted` is routinely
  False for `NoMatchingParent` while rule-level findings are still worth reporting; this would
  break `invalid_cors_results_in_partially_invalid_not_accepted_false`.
- Add a `DelegationIssue` variant to escalate AI drops from delegated children — widens a shared
  `edgion-resources` schema for a case the delegation contract already covers by design.

## Re-evaluation Triggers

Re-open this decision only if:

- The gateway stops honouring `Accepted` when selecting parentRefs (then `Accepted=False` would no
  longer imply a whole-route drop and the gate could be relaxed).
- Gateway API introduces a standard per-rule status surface, making `PartiallyInvalid` obsolete.

## Reference Cases

- `tasks/review-ai-work/ctrl-01-ai-rule-accepted-vs-partially-invalid.md` (2026-08-04)
- `tasks/pending/ai-gateway-resilience-and-routing/01-design.md` §AiProxy attachment,
  `06-decisions.md` #8
- `docs/en/ai-gateway/resilience.md` §AI rule rejection ·
  `docs/zh-CN/ai-gateway/resilience.md`
- `skills/01-architecture/05-resources/04-http-route.md` §update_status
