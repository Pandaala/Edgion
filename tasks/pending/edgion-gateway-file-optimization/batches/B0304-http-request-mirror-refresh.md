# B0304 — HTTP RequestMirror Refresh

## Scope

- Baseline: `8ae7f72f`.
- Pipeline state: analysis-ready at `8ae7f72f`; refresh against current HEAD before promotion.
- Active anchor crate: `edgion-gateway`.
- Cohesive targets: `request_mirror/budget.rs`, `config.rs`, `mirror_log.rs`, `plugin.rs`, and `tests/plugin_test.rs`.
- Exact technical write scope is not approved during analysis. Existing `edgion-tests/**`, campaign records, unrelated modules and user-protected paths are read-only.

## Context and value gate

- Budget/config/tests were added by external `90a45683`; plugin and log ownership were later changed by ReferenceGrant and RequestId migrations. The full module and its immutable publication/runtime boundaries require one cohesive value review.
- The analyst must inspect bounded concurrency/body handling, destination authorization and egress, request cloning, lifecycle/cancellation, canonical trace identity, logging/access-log ownership, failure policy and deterministic tests before deciding change, examined-no-change or defer.

## Analysis result

- Analyst `b0304_request_mirror_analysis` examined all five targets and their body-budget, cancellation, ReferenceGrant, egress, trace, access-log, metrics and failure-isolation call chains.
- Decision: all five targets are examined-no-change candidates. Current limits, request cloning, cancellation ownership, canonical outbound controls and observability remain coherent; no technical write scope is proposed.
- One wider finding requires independent confirmation before task creation: RequestMirror standard fields lack consistent semantic validation across EdgionPlugins and non-Kubernetes HTTPRoute inputs. Its proper scope is resource/controller validation parity, not this Gateway batch.
- The analyst also reported stale RequestMirror observability prose and a potentially stale D0067 tracking entry. These remain coordinator candidates until the independent reviewer verifies the current sources and knowledge records.
- No integration edit or execution is proposed; existing integration files remain frozen.
