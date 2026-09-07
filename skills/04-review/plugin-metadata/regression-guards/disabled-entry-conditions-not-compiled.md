---
name: disabled-entry-conditions-not-compiled
description: Use when reviewing findings that a disabled plugin entry's conditions/config still cause validation errors, or the inverse (a disabled entry should be validated); covers the `enable: false` = semantically-inert contract.
status: fixed
finding_id: plugin-disabled-entry-semantics
closed: 2026-07-18
knowledge_type: regression-guards
---

# Disabled Plugin Entry Conditions Are Not Compiled/Validated — FIXED

## Conclusion

An entry-level `enable: false` on EdgionPlugins is **semantically inert**: it is
not built, not run, and its dormant `conditions` (regex / `timeRange`) and inner
config MUST NOT produce a preparse error or flip the resource to
`Accepted=False`. Both the schema layer (`validate_plugins` Pass 1) and the
Gateway (`compile_plugin_conditions`) now skip disabled entries before compiling
conditions, matching what Pass 2 / `add_from_*` already did for config / alias /
stage / dependency resolution.

Fix suggestions of the form "compile/validate conditions for every entry
regardless of `enable` so problems surface early" are **not accepted** — a parked
entry is a no-op and a no-op cannot be a validation error.

## Core Rationale

**1. Disabled = never executed, so it cannot be a fault**

The Gateway runtime builders (`add_from_*`, `from_stream_plugins`,
`from_entries`) skip `!is_enabled()` entries — a disabled entry is never
constructed and never runs. Compiling its `conditions` was the last remaining
place that still touched a disabled entry, so a parked entry carrying a bad
regex could push a preparse error into `preparse_errors` → the Controller set
`Accepted=False`. That is a false-invalid on config the data plane ignores.

**2. Schema/runtime parity is enforced by a test**

`validate_plugins` (schema) and `compile_plugins` (runtime) must return
identical error sets (`plugin_store.rs` `validate_plugins_matches_compile_plugins_error_set`,
case `disabled_bad_condition`). The skip lives in the two mirror functions;
changing one without the other turns the parity test red.

**3. HTTP now matches the pre-existing stream rule**

EdgionStreamPlugins already treated a parked broken config as inert
(`preparse_skips_disabled_plugins`, covering both `plugins` and
`tlsRoutePlugins`; `stream_disabled_ip_list_ref_reports_error` for refs). HTTP
and stream follow one rule.

**4. Enabled-invalid stays fail-closed — do not conflate**

This does NOT touch the enabled-invalid path. An `enable: true` entry with bad
config still seals the whole resource via `seal_fail_closed` (HTTP 500 per
request). Disabling an auth/authorization plugin intentionally bypasses it;
an invalid *active* plugin must never fail open.

## Fix Suggestions Not Accepted

- "Validate conditions for disabled entries too, to warn operators early" — a
  disabled entry is a no-op; surfacing it as `Accepted=False` is a false-invalid
  and diverges from the stream rule.
- "Make a disabled-with-bad-condition entry fail closed (500)" — bypass is the
  operator's intent; only enabled-invalid fails closed.
- "Skip in the schema layer only (or the gateway only)" — breaks the
  schema/runtime parity test.

## Re-evaluation Triggers

Re-open only if:

- A future requirement wants disabled entries surfaced as a distinct
  non-`Accepted` status condition (e.g. `PartiallyInvalid`) — that is a new
  status-model feature, not this bug.
- The runtime builders start constructing disabled entries for any reason.

## Reference Cases

- Task `tasks/todo/plugin-disabled-entry-semantics.md`, closed 2026-07-18.
- Source: `edgion-resources/src/resources/edgion_plugins/validate.rs` (Pass 1),
  `edgion-gateway/src/plugins/http/plugin_store.rs` (`compile_plugin_conditions`).
- Related: [conditions-timerange-preparse-validation.md](conditions-timerange-preparse-validation.md)
  (the preparse validation this scopes to enabled entries only).
- Doc: `skills/02-features/03-resources/13-edgion-plugins.md` ("`enable: false`
  means bypass, not reject").

## Resolution references

- `plugin-disabled-entry-semantics` — 2026-07-18.
