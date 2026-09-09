# B0122 — Runtime Dye Plan

## Scope

- Baseline: `486e8fea12bd870325b489839262729d14ecb22c`
- Pipeline state: committed
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

- Analysis: defer. The target is already a compact immutable compiled plan with appropriate canonical session/header mutation ownership and no worthwhile local cleanup. A complete call-chain audit found a real wider contract inconsistency instead: dye capabilities are plugin-type-wide while actual outcome production depends on stage and path. Wasm response-body dye can be accepted although its callback leaves the received session unused and records no outcome; after response headers commit, a newly queued response operation would also miss the normal header-drain point. AiProxy records failure/reject outcomes while declaring no dye capabilities/prefix, and ExtProc records response success although its table is failure-only. A local `dye_plan.rs` edit cannot safely decide product behavior.
- Candidate deferred scope: design stage/path/effective-configuration-aware dye capabilities, reconcile Wasm/AiProxy/ExtProc declarations with actual producers and response-header commit timing, add a focused parity guard, and repair missing dye design-record links. No integration issue is required for this record-only batch; any later choice to expose previously unsupported public dye behavior must track its own integration coverage.
- Freshness: analyzed at `486e8fea`; target blob `b7255036a98cc30dea277c89d9c2f631f6027242` remained unchanged and B0121's unrelated evaluator diff was not modified.
- Current-HEAD refresh: revalidated at `8a0f44c3`. B0121's condition publication changes correctly make malformed active dye conditions fail closed and preserve unresolved dye conditions as observational non-matches, but they do not alter the Wasm/AiProxy/ExtProc capability-versus-producer mismatch. The target blob and defer decision remain unchanged, and no duplicate task or review page exists.
- D0083 was deferred in this batch; B0122 made no runtime edit. Its subsequent repair is documented in [dye stage and producer parity](../../../../skills/04-review/plugin-runtime/regression-guards/dye-stage-producer-parity.md), including the outstanding fixture-alignment scope.
- Independent review: confirmed the defer classification, plugin mismatches, missing design authority, D0083 ownership, and no-integration disposition, but required factual precision about the Wasm session/header lifecycle and broader execution-path parity keys. The records were corrected without runtime edits; re-review is pending.
- Final re-review: LGTM with no Must Fix or Should Fix. The reviewer confirmed the corrected Wasm session/header lifecycle, runtime-reachable but renderer-unreachable AiProxy/ExtProc producers, stage/path/effective-configuration parity key, generalized future integration ownership, D0083 classification, and unchanged runtime/frozen tests.
- Verification: no runtime test was needed for this record-only defer. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA force, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen 3,084-file integration guards all passed at `8a0f44c3`; Clippy emitted only the existing warning set.
- Commit: `5a95df60`.
