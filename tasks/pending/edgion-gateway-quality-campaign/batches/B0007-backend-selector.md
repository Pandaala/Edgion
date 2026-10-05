# B0007 — Backend Selector

## Scope

- Baseline: `8a06ebf3b398ac3b12c38b27b9602945d355cb0f`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/backends/selector.rs`
- Allowed code files: `edgion-gateway/src/backends/selector.rs` only.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the code file above and campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0007 analysis turn.
- Confirmed defect: sticky-session address restoration searches canonical all-slot affinity state but computes eligibility only from live health/outlier state. An EndpointSlice endpoint that remains published with `ready=false` can therefore bypass the ordinary ready-only selector.
- Decision: `change` (simple). Keep all-slot token recognition, but require `slot.ready` in the canonical restoration eligibility gate so strict mode can recognize the pinned endpoint as unhealthy and fail closed.
- Preserved semantics: canonical address matching, section scoping, normal ready-only selection, generation/runtime fencing, strict/non-strict persistence behavior, health/outlier eligibility, schema, logs, metrics, and protocol behavior.
- Freshness revalidation: valid at `8a06ebf3b`; B0006 changed neither the selector nor its discovery/runtime/session-persistence call chain.

## Implementation

- Implementation subagent: `b0002_analysis` in a B0007 implementation turn.
- Complexity: simple; no plan-review stage required after freshness validation.
- Actual change: sticky address restoration now combines the canonical slot
  readiness bit with existing live health/outlier eligibility while preserving
  all-slot address recognition.
- Tests added: one deterministic selector regression proves ordinary selection
  rejects an unready-only section, sticky restoration returns its metadata as
  ineligible, and a ready control remains selectable and eligible.

## Independent review

- Reviewer: `b0001_review`, independent from the B0007 analyst and implementer.
- Verdict: approved with no findings. The reviewer confirmed strict mode now
  fails closed through the existing unhealthy-target path, non-strict fallback
  is unchanged, and generation, membership, section, port, and hot-path
  invariants remain intact.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Baseline selector suite | passed | 10 selector tests passed during analysis. |
| Frozen integration paths | passed | Existing integration files remained unchanged during analysis. |
| Focused implementation checks | passed | 11 selector, 21 session-persistence, and 8 runtime-state-focused tests plus Gateway all-target check, formatting, no-tracing, English-only, and diff checks passed. |
| Independent review checks | passed | 11 selector and 49 session-persistence-focused tests plus Gateway lib/fmt/agent-doc/no-tracing/English/diff checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `50c6fe928374d73435582d8f68109df7413a5eff`.
