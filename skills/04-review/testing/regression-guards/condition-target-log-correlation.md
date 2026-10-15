---
name: condition-target-log-correlation
description: Require scoped unique plugin identities and target outcomes in condition integration assertions.
knowledge_type: regression-guards
---

# Condition target log correlation

## Scope and regression boundary

The AllConditions integration suite attaches 19 independent non-terminating Mock
entries to one resource. An aggregate Mock count cannot establish the outcome of
one entry: the 100% probability sibling runs even when the tested cookie or
sampling entry is missing or skipped. A negative assertion must also reject an
empty parsed log; absence is not proof that a configured entry was skipped.

Use the existing entry alias in the canonical access log. The suite's private
observer requires exactly one `Mock` with the selected alias within
`request_filters` and resource `condition-all-types`. Missing and duplicate
targets fail, including duplicates across matching stage/resource containers.
Wrong-scope entries cannot satisfy the target. A run requires no `cond_skip`;
a skip requires the exact current action and cause descriptor. Single-leaf
success does not require match-only condition logs.

The current causes are path-bearing, for example
`skip:skip.anyOf[0]:keyMatch:path` and
`!run:run.allOf[0]:probability`. Keyed probability uses the same value-free
cause as plain probability. Cookie and method accessors are `cookie:session_id`
and `method`. See the [plugin architecture](../../../01-architecture/02-gateway/05-plugin-system.md)
and [access-log contract](../../../03-coding/observability/00-access-log.md).

## Evidence and acceptance

ARCH-383 replaced aggregate counts, substring searches and unconditional negative
fallbacks with the scoped target observer in
`edgion-tests/src/client/suites/edgion_plugins/plugin_condition/all_conditions.rs`.
The two `EdgionPlugins_default_condition-all-types.yaml` fixture copies use the
same 19 aliases. Their three exclusion predicates match directly under `skip`;
`negate: true` would invert the documented /health, OPTIONS and /debug behavior.
This is a fixture correction, not a change to condition evaluator semantics.

The 30 case identities and original request vectors remain the coverage surface.
Every sample validates its own target: 100% uses three requests, 0% one, and
same-key deterministic probability five. Comparing target booleans detects
variation even while an unrelated sibling runs on every request.

Pure regressions in the module exercise missing and duplicate identities,
wrong scopes/types, contradictory outcomes, wrong/legacy causes, target
variation and a missing final sample. The fixture regression checks complete
parsed parity, the alias inventory and exclusion polarity. Run them with
`cargo test -p edgion-tests --lib all_conditions`. These pure checks do not prove
real dispatch, config publication or log-store delivery; the existing
PluginCondition/AllConditions suite must also run through the sole shared
integration topology. Consult the task verification record for actual executed
results; this page does not claim a fresh integration pass.

## Re-evaluation triggers

Revisit target scope if the resource attaches more than once per request, stage
identity changes, or aliases acquire a different ownership boundary. Revisit
cause expectations when the canonical compiled descriptor changes. Never repair
an assertion by adding request diagnostics, weakening log uniqueness, changing
production condition semantics, or restoring aggregate success fallbacks.
