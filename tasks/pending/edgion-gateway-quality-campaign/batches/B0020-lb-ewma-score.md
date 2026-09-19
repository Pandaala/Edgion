# B0020 — LB Peak-EWMA Score

## Scope

- Baseline: `40c18a076b635ff23c31fbda291d21d970275bf5`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/lb/ewma/score.rs`
- Allowed files: the anchor plus `skills/12-algorithms/lb-peak-ewma.md`.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the two implementation/knowledge files above and B0019/B0020 campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0020 analysis turn.
- Confirmed defect: response callbacks can capture a timestamp and publish after a newer observation. Both writers recomputed values on CAS retry but stored the stale caller timestamp, allowing a delayed observation to move the decay anchor backward and prematurely relax an above-default penalty.
- Decision: `change` (complex because it reverses an explicitly accepted algorithm-test decision and updates load-bearing canon). Within each CAS iteration, derive a non-regressing pre-wrap anchor from the latest packed timestamp and store that same anchor.
- Preserved semantics: one packed `AtomicU64`, Relaxed lock-free operations, peak-immediate behavior, pure reads, fixed-point decay, timestamp units/constants/defaults, zero allocation, no extra clock read, and the documented multi-year u32 wrap limitation.
- Freshness revalidation: valid at `40c18a076`; B0019 changed backend connection-counter release and campaign records only.

## Plan review

- Implementer plan: `b0002_analysis` produced a plan without editing.
- Independent plan reviewer: `b0001_adjudicate`.
- Verdict: approved. The reviewer confirmed stale strict peaks may conservatively raise the value without regressing the anchor, stale faster samples retain zero weight, peak-only booleans remain exact, and the wrap limitation must remain explicit.

## Implementation

- Implementation subagent: `b0002_analysis` after plan approval.
- Actual change: added a pure anchor/elapsed helper used inside every CAS iteration of both writers; packed writes use the non-regressing anchor. Direct packed-state tests cover stale normal and peak-only higher/equal/lower observations.
- Documentation: pseudocode, implementation invariants, and post-wrap freeze behavior now match the code.
- Plan deviations: none.

## Independent review

- Reviewer: `b0001_review`, independent from the B0020 analyst, implementer, and plan reviewer.
- Verdict: approved with no findings. The reviewer verified CAS retry recomputation, chronology, booleans, arithmetic, atomics, hot-path characteristics, documentation, and explicit retention of the wrap limitation.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused implementation checks | passed | Score 21, sample 8, response-filter sampling 5, and compiled-selector 5 tests plus Gateway all-target check, formatting, agent-doc, no-tracing, English-only, diff, and frozen checks passed. |
| Independent review checks | passed | Score, sample, response-filter, compiled-selector, and backend-selector suites plus Gateway library check/clippy, formatting, agent-doc, no-tracing, and diff checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
