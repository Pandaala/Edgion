---
name: chain-ordinary-work-accumulates-per-unit
description: Use when reviewing native WAF rule-unit evaluation, chain frontier branches, and ordinary operator work accounting.
knowledge_type: regression-guards
---

# Chain ordinary work accumulates across members and branches

## Conclusion and scope

In `edgion-waf/src/seclang/transaction/units.rs` (with the field declared in
`transaction/mod.rs` and `charge_stage` in `transaction/predicate.rs`), ordinary operator work (original
bytes, transformed bytes and detector bytes charged by
`OrderedStageAccounting::charge_stage`, plus JSON tail logical work) is
accumulated in the transaction field `unit_work_bytes` for one rule unit.
`evaluate_unit` resets it once per unit; every `evaluate_member_matches_into`
call for that unit starts from the accumulated value and stores it back on
success, early stop, and error. The JSON no-hit tail fast path includes the
carried unit work in its admission proof and otherwise falls back to the
cursor path. The budget therefore survives every chain
member and every frontier branch, including branches whose final member fails.
The bound is the existing `RuntimeLimits::max_operator_work_bytes`, and
exhaustion returns the existing terminal `RuntimeError::OperatorWorkLimit`.

Before transforming a materialized selected value, the member loop rejects the
value when the accumulated work plus its original length would exceed the
limit, because every subsequent stage charge includes those original bytes.

A single-member rule keeps its previous bound. A chain now shares one bound
across all members instead of receiving one per member invocation.

## Rationale and evidence

The former local `work = 0` per member invocation let a chain whose first
member matched many client-supplied values re-run an expensive,
branch-independent later member (for example transformations and `@rx` over a
large header or `REQUEST_BODY`) once per branch, each invocation staying within
its own limit. Frontier checks bound only live branch count and branch bytes,
not work already consumed, and materialized bodies bypass the borrowed
transform memo and non-matcher operators bypass the ordinary scan cache. Work
was therefore multiplied by client-controlled fan-out.

Unit-level accumulation removes the client-controlled multiplier: total
ordinary work per phase is bounded by the configured rule-unit count times the
per-unit limit, which is operator-controlled. Detector work keeps its separate
transaction-wide counter (see
[native injection operators](native-injection-operators-use-bounded-detection.md)).
The request-body prefix path evaluates only single-member units and keeps its
own phase counter; stream member results are precomputed booleans.

## Alternatives and limits

A transaction-wide ordinary work budget (new `RuntimeLimits` field) was
considered and not adopted in this fix: it would cap the whole ruleset, needs
calibration for CRS-scale rulesets on large bodies, and would change behavior
for every deployment. The per-unit bound is not a CPU-time or wall-clock limit,
and one value's transformation work before its charge remains bounded only by
the transformation limits (JSON/XML selected values have no pre-check).
Known cost: with default limits (64 MiB work, 16 MiB input), a legitimate chain
whose two members each inspect a near-maximum body can now fail closed where
each member previously had its own budget. No CRS-scale or large-body
calibration was performed; most CRS chain tails inspect TX, MATCHED_VAR or
small headers. Residual: per phase, total work can still reach the evaluated
unit count times the per-unit limit; this is operator-controlled, and a
transaction-wide budget would be a separate design task. Chain frontier byte and evaluation limits are
unchanged and independent.

## Verification and re-evaluation

Implementation run on 2026-11-05: `cargo test -p edgion-waf --lib ordinary_work`
passed `ordinary_work_accumulates_across_failed_chain_branches` (five failed
branches exceed a limit one branch fits; no staged TX variable is published)
and `ordinary_work_budget_resets_for_each_rule_unit`. The first test fails with
the previous per-invocation counter. No Gateway timing measurement was made.
Re-evaluate when rule-unit scheduling, chain frontier evaluation, the stage
accounting observer, the JSON summary/no-hit tail admission, or a
transaction-wide work budget changes, or when large-body chains are reported to
fail closed.

## Resolution references

- `ISSUE-011` (run `e6cbd068`, baseline `e6cbd0688eb0`): "WAF chain evaluation
  resets ordinary work accounting across branches", confirmed high by static
  evidence; fixed by per-unit cumulative ordinary work on 2026-11-05.
