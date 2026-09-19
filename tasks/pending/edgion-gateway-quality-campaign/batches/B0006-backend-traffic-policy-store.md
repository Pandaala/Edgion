# B0006 — Backend Traffic Policy Store

## Scope

- Baseline: `b47f6b0acd0cf99c2e973648821c60f0580a923f`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/backends/policy/backend_traffic/store.rs`
- Allowed code files: `edgion-gateway/src/backends/policy/backend_traffic/store.rs`, `edgion-gateway/src/ai/credential_outlier.rs`, and `edgion-gateway/src/ai/backend_store.rs`.
- Forbidden files: every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the three code files above and campaign ledger/batch/main/changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in B0006 analysis and freshness turns.
- Confirmed defect: AI credential-outlier policy publication and the effective EBTP ArcSwap become visible in different orders on add and removal. A request that captured no effective EBTP policy can therefore use a newly unpublished or already removed configured outlier generation through the required live fallback.
- Decision: `change` (complex, plan-first). Keep one canonical per-target credential-outlier entry, but distinguish its config-free fallback generation from the effective EBTP policy generation captured by `EffectiveBackendTrafficPolicy`.
- Preserved semantics: fallback credential admission and provider cooldown, alias reconciliation, captured Arc lifetime, equivalent-policy identity/state, retry/circuit-breaker/timeout fields, schema, logs, metrics, failure policy, and protocol behavior.
- Freshness revalidation: valid at `b47f6b0ac`; B0005 did not touch the three allowed code files or alter this publication boundary.

## Implementation

- Implementation subagent: `b0002_analysis` in separate plan and implementation turns.
- Complexity: complex; implementation plan and independent plan review required.
- Approved plan: keep one canonical mutable credential-outlier entry per AI
  target with distinct config-free fallback and effective-policy generations.
  Backend credential alias changes use copy-on-write fallback generations that
  share retained credential state; effective generations reconcile aliases
  live and preserve identity for equivalent policy configurations.
- Immutable publication: each `AiBackendSnapshot` captures a derived
  target-to-fallback-Arc projection alongside its runtime backend map. Request
  planning performs only an immutable map lookup and Arc clone, so old snapshots
  retain their generation and production code never performs an ambiguous live
  effective-or-fallback lookup.
- Actual implementation: completed exactly within the approved three-file
  scope. Config-free fallback aliases now advance through copy-on-write
  generations, effective policy generations remain separately owned, and the
  backend ArcSwap publishes its runtime map and fallback projection together.
- Tests added or corrected: deterministic role/add/removal-gap coverage,
  equivalent/change identity, alias reconciliation and cleanup, and old/new
  backend snapshot isolation with retained cooldown state.

## Plan review

- Reviewer: `b0001_adjudicate`, independent from the B0006 analyst and implementer.
- Initial verdict: changes requested. The first plan retained a request-path
  DashMap lookup and a production-ambiguous `current_handle` API.
- Final verdict: approved after moving fallback capture into the immutable
  backend snapshot, using copy-on-write fallback generations, and restricting
  combined lookup/configuration helpers to tests.

## Plan review

- Reviewer: pending.
- Verdict: pending.

## Independent review

- Reviewer: `b0001_review`, independent from the B0006 analyst, implementer,
  and plan reviewer.
- Verdict: approved with no findings. The reviewer confirmed the publication
  gap is closed, request planning is immutable and lock-free with respect to
  the canonical store, COW state sharing is safe, lifecycle cleanup works in
  either order, and all preserved semantics remain intact.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths | passed | Recorded baseline remained unchanged through analysis and freshness validation. |
| Focused implementation checks | passed | Credential-outlier, backend-store, EBTP-store, AI attempt, dispatch, and proxy suites; Gateway all-target check; formatting, no-tracing, English-only, and diff checks passed. |
| Independent review checks | passed | 16 credential-outlier, 21 backend-store, 26 EBTP-store, 14 AI proxy, and 23 dispatch tests plus Gateway lib/fmt/agent-doc/no-tracing/English/diff checks passed. |
| Correction and incremental review | passed | Replaced one new `let...else` clippy warning with equivalent `?`; full workspace clippy and 21 backend-store tests passed, and the independent reviewer re-approved. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
