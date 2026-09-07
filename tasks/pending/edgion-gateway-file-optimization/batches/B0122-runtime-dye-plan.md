# B0122 — Runtime Dye Plan

## Scope

- Baseline: `486e8fea12bd870325b489839262729d14ecb22c`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/dye_plan.rs`
- Baseline blob: `b7255036a98cc30dea277c89d9c2f631f6027242`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete dye-plan implementation, its resource schema and validation, compilation/publication callers, request/response execution sites, condition evaluation, response-header mutation, tests, and relevant review knowledge.
- Preserve immutable-generation ownership, active/policy-stamped entry semantics, condition ordering, header validation and set/append behavior, canonical access-log ownership, redaction, observability restraint, bounded request-path work, and response streaming.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: defer. The target is already a compact immutable compiled plan with appropriate canonical session/header mutation ownership and no worthwhile local cleanup. A complete call-chain audit found a real wider contract inconsistency instead: dye capabilities are plugin-type-wide while actual outcome production depends on stage and path. Wasm response-body dye can be accepted although its body callback cannot record an outcome; AiProxy records failure/reject outcomes while declaring no dye capabilities/prefix; ExtProc records response success although its table is failure-only. A local `dye_plan.rs` edit cannot safely decide product behavior.
- Candidate deferred scope: design stage/path-aware dye capabilities, reconcile Wasm/AiProxy/ExtProc declarations with actual producers and response-header commit timing, add a focused parity guard, and repair missing dye design-record links. No integration issue is required for this record-only batch; a later choice to expose AiProxy dye as a feature should track its own integration coverage.
- Freshness: analyzed at `486e8fea`; target blob `b7255036a98cc30dea277c89d9c2f631f6027242` remained unchanged and B0121's unrelated evaluator diff was not modified.
- Review: pending.
- Verification: pending.
- Commit: pending.
