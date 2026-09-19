# B0018 — LB Compiled Selector

## Scope

- Baseline: `ff372c88299b2992588b5f9665c2aeca40b72c34`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/lb/compiled_selector.rs`
- Allowed source files: none; source analysis concluded examined-no-change.
- Allowed knowledge files: move `skills/04-review/lb/intentional-design/build-lock-single-mutex-intentional.md` to `skills/04-review/lb/superseded/build-lock-single-mutex-intentional.md`, plus `skills/04-review/lb/SKILL.md`, `skills/04-review/concurrency/intentional-design/webhook-manager-rwlock-intentional.md`, and `skills/04-review/backends/regression-guards/ep-slice-no-port-fail-closed.md`.
- Forbidden files: every Rust source file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the five knowledge paths above and B0017/B0018 campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0018 analysis turn.
- Source decision: `examined-no-change`. Each service section eagerly builds all policy selectors from one immutable ready-backend slice and publishes them with the exact slot generation. Degradation, health, Arc identity, and bounded fallback behavior are coherent.
- Confirmed knowledge defect: active lb-F010 guidance described the deleted lazy `ServiceSelectorCell` and coarse `build_lock` model as current. An active no-port EndpointSlice guard also retained that retired hydration path and the wrong empty-section error mapping.
- Decision: `change` (simple, knowledge-only). Reclassify lb-F010 as superseded, repair inbound routing, and align the active EndpointSlice guard with eager coordinator publication and current selector errors.
- Preserved semantics: all runtime selector/degradation behavior, request observability ownership, schema/transport, TLS, and frozen integration behavior.
- Freshness revalidation: valid at `ff372c882`; B0017 changed WRR arithmetic and campaign records only.

## Implementation

- Implementation subagent: `b0002_analysis` in B0018 implementation and correction turns.
- Complexity: simple; no plan-review stage required.
- Actual change: moved/reworked lb-F010 as historical superseded guidance, updated its index and webhook sibling link, and rewrote the active no-port guard to the eager `build_section` and request selector path with verified error mappings.
- Reviewer correction: the first review found the active EndpointSlice guard as an additional same-cause stale path; the allow-list expanded by exactly that file before re-review.

## Independent review

- Reviewer: `b0001_review`, independent from the B0018 analyst and implementer.
- Initial verdict: requested changes for the active EndpointSlice guard's retired hydration path and wrong default-RR empty-section error.
- Final verdict: approved with no findings after correction. The reviewer verified current publication/selection behavior, all three error mappings, historical labeling, link routing, and examined-no-change source verdict.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Analysis checks | passed | Compiled-selector, backend-selector, and discovery-coordinator suites passed. |
| Implementation checks | passed | Agent-doc, English-only, reference, diff, exact-scope, and frozen checks passed. |
| Independent review checks | passed | Source suites, EndpointSlice facade/inventory tests, agent-doc, English-only, diff, scope, and frozen checks passed before and after correction. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
