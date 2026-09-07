---
name: conditions-timerange-preparse-validation
description: Use when reviewing `TimeRangeCondition::evaluate` / `PluginConditions` findings; covers the closed defect where malformed RFC3339 timestamps were silently ignored at runtime instead of failing closed at preparse.
knowledge_type: regression-guards
---

# TimeRangeCondition RFC3339 preparse validation — FIXED

## Conclusion

The former direct `TimeRangeCondition::evaluate` path wrapping each bound parse in `if let Ok(..) = chrono::DateTime::parse_from_rfc3339(...)` with no `else` arm WAS a fail-open defect (plugins-runtime-F002, 2026-05-26). Activation is now enforced at publication, the **only** correct fail-close location, mirroring how regex compile failures are handled.

`compile_plugin_conditions` (`edgion-gateway/src/plugins/http/plugin_store.rs`) now calls `PluginConditions::validate()` alongside `PluginConditions::compile()`, and `Condition::validate()` dispatches `TimeRangeCondition::validate()` over each non-`None` `after` / `before` string. A parse failure pushes `"<stage>[<i>] conditions: timeRange.<field> '<value>' is not a valid RFC3339 timestamp: <err>"` into `preparse_errors` and separately records that active entry in `PluginRuntime::invalid_plugins`. `compile_plugins()` derives activation and update policy from that runtime state before `seal_fail_closed()` installs the canonical leading HTTP 500 reject gate.

The Controller independently runs `EdgionPlugins::validate_plugins()` and writes the same static diagnostic into the resource status. `preparse_errors` are diagnostic output, not themselves the Gateway activation predicate; keeping Controller/Gateway error parity does not replace the Gateway's runtime-owned enforcement.

> **Location note (2026-06-11, types-decouple):** the condition types + `PluginConditions::validate()` now live in `edgion-resources/src/resources/edgion_plugins/conditions.rs` (schema leaf); the preparse gate that drives them moved from `EdgionPlugins::compile_conditions`/`preparse` (deleted from `types`) to the gateway's `compile_plugin_conditions`/`compile_plugins` in `plugin_store.rs`. The behavior below is unchanged — only the file locations moved.

Fix suggestions of the form "add `tracing::warn!` + `return false` in `TimeRangeCondition::evaluate`" (Option B in the original issue) are **not accepted** — they would catch the error only at first request instead of at preparse, would leave no K8s status visibility, and would diverge from the regex compile-failure handling pipeline.

## Core Rationale

**1. Preparse parity with regex compile failures.** The condition compiler preserves every regex or timestamp diagnostic in `preparse_errors` and marks each affected active entry invalid once, even if its entry, body, and multiple dye conditions all fail. Timestamp parse failures are the same class of operator misconfiguration as regex compile failures. Both therefore produce Controller status diagnostics and, independently, the Gateway's `invalid_plugins` → `seal_fail_closed` enforcement path.

**2. Runtime `tracing::warn!` would be insufficient.** Even if `TimeRangeCondition::evaluate` started logging on the parse-failure branch, the operator would still receive (a) zero feedback during `kubectl apply`, (b) no `status.conditions` entry on the EdgionPlugins, and (c) a one-line warn per request — a noisy per-request channel rather than a definitive "this resource is broken" signal. The `data-plane hot path` Iron Rule 3 also bars adding `tracing::*` to `evaluate` itself.

**3. Runtime parsing is compiled once, with a defensive fallback.** `CompiledCondition::compile` parses TimeRange bounds into UTC once per immutable runtime generation. Request and response-body evaluation samples one `Utc::now()` and performs only comparisons. Canonical validation rejects malformed active bounds before publication; direct defensive construction treats each malformed bound independently as absent, without logging on the hot path. This fallback is not the publication gate and must not be cited as fail-close enforcement.

**4. Unresolved policy remains consumer-specific.** Active entry and secondary body conditions with unresolved references mark the runtime invalid and non-retainable. Dye conditions are observational, so unresolved dye references remain a non-match and do not block activation. A malformed dye TimeRange or regex is still a static configuration error and does block activation.

## Fix Suggestions Not Accepted

- "Add `else { tracing::warn!(...); return false; }` to `TimeRangeCondition::evaluate`" — catches the error at first request, not at preparse; violates Iron Rule 3 on the data-plane path.
- "Treat parse failure as `false` (skip plugin) silently at evaluate time" — flips fail-open to fail-skip without operator feedback; same K8s status visibility gap as the original defect.
- "Validate at deserialize time via a custom `serde` impl on `TimeRangeCondition`" — the project policy is that schema-shape validation happens in preparse so all config errors surface through one channel (`preparse_errors`); the deserializer must stay lenient so partial configs can be inspected.

## Re-evaluation Triggers

Re-open this decision only if:

- A new `PluginConditions` publication path is added that does NOT flow through `compile_plugin_conditions`. The new path must provide both static diagnostics and an explicit consumer-owned activation policy; the runtime evaluator is still not the right place for request-time logging.
- A new `Condition` variant is added that carries an unparsed stringly-typed field. Extend `Condition::validate()` to dispatch to its `validate()` impl; do not put the parser in `evaluate`.

## Reference Cases

- `tasks/plugins/plugins-runtime-F002-time-range-condition-silent-parse-fail.md` (closed 2026-05-26)
- `edgion-gateway/src/plugins/http/plugin_store.rs::compile_plugin_conditions` (preparse gate)
- `edgion-resources/src/resources/edgion_plugins/conditions.rs::{Condition,PluginConditions,TimeRangeCondition}::validate`
- Regression tests: compiled TimeRange construction/boundary tests in `evaluator.rs`, plus condition activation, unresolved-policy, deduplication, and hot-replacement tests in `plugin_store.rs`.

## Resolution references

- `plugins-runtime-F010` — 2026-05-27.
- `plugins-runtime-F002` — 2026-05-26.
