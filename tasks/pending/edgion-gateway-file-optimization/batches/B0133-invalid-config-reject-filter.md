# B0133 — Invalid Configuration Reject Filter

## Scope

- Baseline: `b71f37f2fe8b3ae837bcfb08e159d540a0b97715`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/reject_filter.rs`
- Baseline blob: `8a6c43516329d3c9e813f8d2784469d0f28da108`
- Implementation scope: not approved; read-only candidate only
- Protected: completed B0128/B0129 worktree changes, all existing integration tests, and unrelated user paths

## Analysis boundary

Read the complete reject filter and trace invalid configuration collection, runtime sealing/publication, HTTP/gRPC rejection, plugin/access-log diagnostics and tests. Verify original failure and redaction requirements. Decide change, examined-no-change or defer from concrete evidence; avoid duplicate findings already owned by D0079/D0089/D0095.

## Handoff

- Analysis: `b0132_policy_analysis` (reused read-only role after the tool thread limit) recommends a simple documentation-only change in this file. The module header incorrectly describes CRD admission as the primary semantic rejection gate and omits native HTTPRoute consumers and store-owned last-known-good retention.
- Proposed scope: only module/response-constant documentation; preserve every runtime byte, signature, log entry and visibility. Describe advisory Controller validation, runtime-selected fail-closed rejection and canonical HTTP/gRPC delivery. No new tests needed for comments only.
- Evidence: plugin CRD preserves semantic errors/config fields for status visibility; native rule/backend ExternalAuth constructors also seal this filter. Existing direct tests prove fixed 500/body/static diagnostics. Do not break after a failed log append: later shorter entries may still fit.
- Knowledge: lenient-admission design rules, fail-closed generic-body canon, native ExternalAuth sealing regression guard, plugin builder-duplication precedent, plugin-system, log safety, access-log and unit-testing guidance. No new deferred finding.
- Review: pending promotion after the active serial batch.
- Verification: pending.
- Commit: pending batch completion; local commit only, never push.
