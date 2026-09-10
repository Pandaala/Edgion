---
name: ai-guard-ordering-publication
description: Review AiGuard/AiProxy same-resource ordering validation, fail-closed publication, and hot-update recovery.
knowledge_type: regression-guards
---

# AiGuard ordering errors seal the resource before request execution

## Conclusion and scope

An active AiGuard after an active AiProxy in one EdgionPlugins request stage
invalidates that compiled resource. `collect_ai_guard_ordering_errors` remains
the sole ordering-policy authority for Controller and Gateway. Disabled and
PluginPolicy-stamped entries are inert for this check; runtime conditions do not
exempt entries from static ordering validation.

`compile_plugins` records one `AiGuardOrdering` identity per invalid resource,
independent of how many entries violate the order. It sets `is_activatable=false`
and `retain_previous_on_invalid_update=false` before sealing. The existing
`RejectInvalidConfig` gate returns a generic HTTP 500 before the resource's
request plugins, including body capture, AI planning and evaluator calls.
Existing PluginPolicy block precedence remains intact.

## Rationale and evidence

- Authority: the selected D0082 task required publication-time rejection; the
  user authorized its correction. The documented AiGuard-before-AiProxy contract
  is in `docs/en/user-guide/http-route/filters/edgion-plugins/ai-guard.md`.
- `edgion-resources/src/resources/edgion_plugins/validate.rs` compares the last
  active guard with the first active proxy and emits one resource diagnostic.
- `edgion-gateway/src/plugins/http/plugin_store.rs::compile_plugins` observes
  diagnostics added by that exact helper, without duplicating its predicate or
  treating unrelated preparse diagnostics as activation failures.
- `PluginRuntime::record_invalid_ai_guard_ordering` records a static, non-secret
  resource identity through `InvalidPlugins::record_non_retainable`.
  `ReplaceFailClosed` dominates mixed DSL/WAF `RetainPrevious` failures.
- `PluginStore::{replace_all,apply_delta}` publish the incoming sealed runtime
  for initial/add/update/reconnect cases. Corrected incoming configurations
  recover normally. Requests already holding older runtime Arcs keep them;
  this change does not revoke in-flight generations.

## Alternatives and limits

Previously the ordering diagnostic entered only `preparse_errors`, leaving
`invalid_plugins` empty and allowing normal activation. Error-set parity tests
did not cover the publication gate. AiGuard already rejected a request when it
actually ran after AiProxy had installed a plan; therefore the original claim
that every misordered request necessarily leaked an unguarded prompt was too
broad. That local check does not replace resource-level enforcement, especially
when conditions skip the guard. No production leakage incident was established.

This check is intentionally same-resource. It does not prove cross-resource
ordering or guard execution when operators intentionally use conditions to skip
a correctly ordered guard. Existing body-generation protection is unchanged.
No new error body, metric, tracing, request-state field or failure-policy knob
is introduced.

## Verification and re-evaluation

Focused `plugin_store` tests cover single/multiple violations, conditional skips,
one redacted rejection identity, disabled/bypassed/blocked entries, valid and
single-sided lists, first publication, full/delta/add replacement and recovery,
mixed DSL/WAF retention precedence, and PluginPolicy block precedence.

Verified on 2026-09-10:

- `cargo test -p edgion-gateway --lib plugins::http::plugin_store::compile_plugins_tests`:
  31 passed, including all five new `ai_ordering_*` tests and existing diagnostic
  parity/publication tests. The earlier four-test targeted run also passed.
- `cargo fmt --all` and the final `cargo fmt --all -- --check` passed.
- `cargo check --workspace --all-targets` and
  `cargo clippy --workspace --all-targets` passed. Clippy emitted existing
  warnings; none pointed to the new ordering code or tests.
- Agent-doc consistency, SSA-force, metrics inventory, Pingora source policy,
  Gateway tracing boundary and `git diff --check` passed.
- Independent read-only review: LGTM. No full workspace test suite or real
  integration run was performed.

Existing integration files are frozen for this task. The remaining real
Controller-to-Gateway hot-reload and provider-observation coverage is tracked in
`tasks/todo/ai-guard-ordering-hot-reload-integration.md`.

Re-evaluate when the shared validator emits different failure categories,
publication retention policy changes, request-stage gate precedence changes,
or ordering support extends across resources.

## Resolution references

- D0082 / `fail-closed-ai-guard-proxy-order-violations` — fixed and verified
  2026-09-10; integration coverage remains separately tracked above.
