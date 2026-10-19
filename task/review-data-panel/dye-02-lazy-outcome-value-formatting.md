# Decide lazy dye outcome value formatting

Status: deferred for user decision. Scope: formatting selected upstream values
for the canonical entry-level dye recorder.

## Evidence

DynamicExternalUpstream formats domain:port after acquiring the jump slot;
DynamicInternalUpstream formats namespace/name when a namespace exists. Both
then call PluginSession::record_outcome. The production session adapter returns
immediately when no current dye plan is installed, so these successful selections
can build and discard a String even when dye is absent.

In `edgion-gateway/src/plugins/runtime/session_adapter.rs`, record_outcome
processes request rules and then response rules. Each rule checks outcome
membership and live conditions before rendering. CompiledDyeRule.value is an
Option<CompiledTemplate>: Some evaluates its template and ignores the recorded
raw text; None uses the raw text, sanitized directly for Success and prefixed
for failure outcomes. A plan can therefore exist without any rule consuming the
formatted selection. Matching template rules must still render and write.

## Decision

Compare a shared lazy-formatting interface, configuration-time preparation of
static target text, and the current implementation. A lazy interface must stay
object-safe and synchronous, with an explicit formatter lifetime and consistent
raw value across one record. Avoid allocating a closure or formatting separately
for every matching rule. Configuration preparation trades request work for
persistent per-target text and only fits values known at construction.

Do not add plugin-specific copies of dye subscription/condition logic. The
canonical recorder must continue owning observation, and default/custom session
implementations need a deliberate interface contract. Measure absent-plan,
unmatched-outcome, failed-condition, template-override and multiple-rule cases,
including allocation and persistent-memory costs. No measured benefit or chosen
runtime design is established by this task.

## Required acceptance

- Preserve jump-slot outcomes, Host mutation order, and exact selected value
  bytes. Rejected jumps must not emit a successful dye record. Fixed failure
  causes and existing unformatted values retain their semantics.
- Preserve request-then-response rule order and evaluate each condition/template
  against live state at its existing point. Later rules may observe earlier
  writes; do not pre-evaluate all rules to decide whether formatting is needed.
- Template overrides still render/write even if raw text is never demanded.
  Shared field sanitization, outcome prefixes, request replacement and response
  Success Set versus failure Append remain canonical.
- Preserve installed entry-plan scope, anti-spoof stripping, response queuing or
  bound-header mutation, local-reply handling and header-commit boundaries.
  Add no late body-stage dye capability, extra logs or secret-bearing output.
- Test real adapter absent/template/raw-value paths and producer outcomes;
  mock convenience must not force an owned copy on the request path. Review
  object safety and any trait/call-site changes under an exact implementation
  scope before execution.

Sources: `plugins/http/dynamic_external_upstream/plugin.rs`,
`plugins/http/dynamic_internal_upstream/plugin.rs`,
`plugins/runtime/traits/session.rs` and `plugins/runtime/session_adapter.rs`
under edgion-gateway/src. Canonical rules are
[entry-level dye](../../skills/01-architecture/02-gateway/entry-level-dye.md) and
[stage/producer parity](../../skills/04-review/plugin-runtime/regression-guards/dye-stage-producer-parity.md).
This task does not change runtime code, run new runtime tests, or supersede
[dye request-name ownership](dye-01-request-name-ownership.md).
