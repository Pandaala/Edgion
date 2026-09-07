# B0121 — Shared Condition Evaluation

## Scope

- Baseline: `86ea7498`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned files:
  - `edgion-gateway/src/plugins/runtime/conditions/evaluator.rs`
  - `edgion-gateway/src/plugins/runtime/conditions/extract.rs`
  - `edgion-gateway/src/plugins/runtime/conditions/mod.rs`
- Baseline blobs: `d566561c44b04b81cec959d460e503809775854c`, `4c89a10c97ff68d77449001d6a6fdd24068c257f`, and `af8443feb45d38e858b3a4766a89eb0aaf836c21`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete evaluator, extractors, module facade, shared resource `Condition` schema/validation, every plugin/runtime consumer, hot-path ownership, tests, and review knowledge.
- Preserve exact AND/OR/negation and missing-value semantics, canonical `Condition` ownership, client/direct peer IP trust boundaries, key/template extraction, redaction, bounded work, observability restraint, and immutable publication.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change in `evaluator.rs`; `extract.rs` and `mod.rs` are examined-no-change candidates. `TimeRangeConditionExt::evaluate` reparses configured RFC3339 `after`/`before` bounds on every condition evaluation because `CompiledPredicate::Sync` retains the raw `Condition`; response-body conditions repeat this work for every upstream chunk. Extraction semantics, canonical KeyGet/session ownership, DirectPeerIp/RemoteIp separation, and the module facade are otherwise correct.
- Candidate correction: add a private compiled time-range predicate during immutable runtime construction, parse each optional bound once, and leave the hot path with one `Utc::now()` plus inclusive-after/exclusive-before comparisons. Preserve defensive invalid-bound behavior by compiling invalid values as absent even though canonical validation rejects them before normal publication. Reuse one comparison helper for compiled and schema/test paths.
- Preserved behavior: AND/OR/negation and unresolved/missing semantics, `after` inclusive and `before` exclusive boundaries, descriptor behavior, condition ordering, no logs/tracing/public API/schema/cross-crate changes, and every client-IP/KeyGet/body/observability invariant.
- Complexity: complex because this shared evaluator runs on request and per-chunk response paths; exact implementation allow-list is only `edgion-gateway/src/plugins/runtime/conditions/evaluator.rs`, including inline tests. Require a written implementation plan and independent plan review before edits.
- Freshness: analyzed at `86ea7498`; all three target blobs exactly match the recorded baselines.
- Deferred disposition: D0080 owns the distinct broader per-chunk owned extraction/JSON/certificate-copy question and must preserve time/probability/ctx mutation semantics. The existing response-body no-work gate task covers outer async-hook overhead rather than extraction and is cross-referenced, not duplicated.
- Integration disposition: existing frozen PluginCondition integration scenarios already cover valid and expired TimeRange behavior. This equivalent internal optimization needs focused unit tests, not a new integration issue.
- Review: pending.
- Verification: pending.
- Commit: pending.
