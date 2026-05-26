---
name: conditions-timerange-preparse-validation
description: Use when reviewing `TimeRangeCondition::evaluate` / `PluginConditions` findings; covers the closed defect where malformed RFC3339 timestamps were silently ignored at runtime instead of failing closed at preparse.
---

# TimeRangeCondition RFC3339 preparse validation — FIXED

## Conclusion

`TimeRangeCondition::evaluate` (`src/core/gateway/plugins/runtime/conditions/evaluator.rs:302-326`) wrapping each bound parse in `if let Ok(..) = chrono::DateTime::parse_from_rfc3339(...)` with no `else` arm WAS a fail-open defect (plugins-runtime-F002, 2026-05-26). It is now fixed at the preparse layer — the **only** correct location, mirroring how regex compile failures are handled.

`EdgionPlugins::compile_conditions` (`src/types/resources/edgion_plugins/mod.rs`) now calls `PluginConditions::validate()` alongside `PluginConditions::compile()`, and `Condition::validate()` dispatches `TimeRangeCondition::validate()` which re-runs `chrono::DateTime::parse_from_rfc3339` on each non-`None` `after` / `before` string. A parse failure pushes `"<stage>[<i>] conditions: timeRange.<field> '<value>' is not a valid RFC3339 timestamp: <err>"` into `preparse_errors`, which `preparse()` forwards to `seal_fail_closed`; every request through that EdgionPlugins is then rejected with HTTP 500 and the Controller writes the message into the resource's K8s status condition.

Fix suggestions of the form "add `tracing::warn!` + `return false` in `TimeRangeCondition::evaluate`" (Option B in the original issue) are **not accepted** — they would catch the error only at first request instead of at preparse, would leave no K8s status visibility, and would diverge from the regex compile-failure handling pipeline.

## Core Rationale

**1. Preparse parity with regex compile failures.** The existing `compile_entries!` macro in `mod.rs` already drives every `conditions.compile()` regex failure through `preparse_errors` → `seal_fail_closed` → `RejectFilter` (HTTP 500) + K8s status condition. Timestamp parse failures are the same class of operator misconfiguration (mistyped CRD value, no client involvement) and must surface through the same channel; adding `Condition::validate()` keeps the dispatch symmetric across condition variants without duplicating the macro.

**2. Runtime `tracing::warn!` would be insufficient.** Even if `TimeRangeCondition::evaluate` started logging on the parse-failure branch, the operator would still receive (a) zero feedback during `kubectl apply`, (b) no `status.conditions` entry on the EdgionPlugins, and (c) a one-line warn per request — a noisy per-request channel rather than a definitive "this resource is broken" signal. The `data-plane hot path` Iron Rule 3 also bars adding `tracing::*` to `evaluate` itself.

**3. No runtime backstop needed.** With the preparse gate in place, no construction path of a `TimeRangeCondition` that reaches `evaluate` can carry an unparseable timestamp — `EdgionPlugins::preparse` is the single entry into `compile_conditions`, and `PluginConditions` only lives inside `EdgionPlugins` entries (no routes hold it independently). Adding a `tracing::warn!` + fail-closed in `evaluate` as defense-in-depth would be dead code in practice and would re-introduce the Iron Rule 3 violation. Tests construct `TimeRangeCondition` directly with known-good strings; if a future test wants to assert the malformed-string evaluation behavior, it should call `validate()` (which IS the runtime semantics now), not bypass preparse.

## Fix Suggestions Not Accepted

- "Add `else { tracing::warn!(...); return false; }` to `TimeRangeCondition::evaluate`" — catches the error at first request, not at preparse; violates Iron Rule 3 on the data-plane path.
- "Treat parse failure as `false` (skip plugin) silently at evaluate time" — flips fail-open to fail-skip without operator feedback; same K8s status visibility gap as the original defect.
- "Validate at deserialize time via a custom `serde` impl on `TimeRangeCondition`" — the project policy is that schema-shape validation happens in preparse so all config errors surface through one channel (`preparse_errors`); the deserializer must stay lenient so partial configs can be inspected.

## Re-evaluation Triggers

Re-open this decision only if:

- A new `PluginConditions` construction path is added that does NOT flow through `EdgionPlugins::compile_conditions` (e.g. route-level conditions become a thing). In that case the new call site must also invoke `PluginConditions::validate()` — the runtime evaluator is still not the right place.
- A new `Condition` variant is added that carries an unparsed stringly-typed field. Extend `Condition::validate()` to dispatch to its `validate()` impl; do not put the parser in `evaluate`.

## Reference Cases

- `tasks/plugins/plugins-runtime-F002-time-range-condition-silent-parse-fail.md` (closed 2026-05-26)
- `src/types/resources/edgion_plugins/mod.rs::EdgionPlugins::compile_conditions` (preparse gate)
- `src/core/gateway/plugins/runtime/conditions/types.rs::{Condition,PluginConditions,TimeRangeCondition}::validate`
- Regression tests: `test_time_range_validate_rejects_malformed_{after,before}` + `test_time_range_validate_accepts_{well_formed_bounds,unset_bounds}` in `src/core/gateway/plugins/runtime/conditions/evaluator.rs`
