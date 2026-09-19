# B0030 — LB Round Robin

## Scope

- Baseline: `8497cfe2c5046b745cedc239dff1ad24d70c9ee6`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- File: `edgion-gateway/src/lb/selection/round_robin.rs`
- Approved knowledge support: `skills/04-review/lb/regression-guards/rr-consistent-hash-per-backend-weight-cap.md`
- Forbidden files: every other source or knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the source and knowledge files above plus B0029/B0030 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0030 analysis turn.
- Decision: `change`, simple, for two local defects.
- Arithmetic defect: post-GCD weights were summed as `usize` before bounded scaling. Coprime extreme weights could panic in debug or wrap in release and bypass the intended schedule allocation cap.
- Fallback defect: weighted primary selection used the expanded schedule, but health fallback started from the raw cursor modulo backend count. Unequal weights could make those positions differ, recheck the rejected primary, and miss a healthy peer within the distinct-backend attempt budget.
- Preserved semantics: sorting, GCD/proportional scaling, zero-to-one normalization, 65,535-backend ceiling, once-only warning and counter, relaxed cursor and wrap, empty/singleton behavior, clones, health policy, and immutable compiled generations.
- Freshness revalidation: valid at `8497cfe2c`; B0029 changed only P2C and campaign records.

## Implementation

- Accumulated and scaled normalized weights in `u128`, then safely returned the bounded values to the existing `usize` schedule expansion.
- Anchored fallback traversal at the actual selected backend index while preserving the total attempt budget.
- Added extreme coprime-weight and weighted cursor/primary mismatch regressions; corrected the existing knowledge page to describe widened accumulation accurately.

## Independent review

- Reviewer: `b0001_review`, independent from the B0030 analyst and implementer.
- Verdict: approved with no findings after proving arithmetic bounds and casts, aggregate schedule bound, fallback coverage/no-duplication, cursor/concurrency preservation, regression effectiveness, observability continuity, knowledge accuracy, and exact scope.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | RR 16, complete selection 36, and compiled-selector 8 tests passed. |
| Independent review checks | passed | Gateway check/Clippy, formatting, agent docs, English-only, diff, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
