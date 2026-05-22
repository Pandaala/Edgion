---
name: force-wildcard-no-tracing-warn
description: Use when reviewing CORS plugin findings that request `tracing::warn!` on `allow_origins='**'` / `allow_methods='**'` / `allow_headers='**'`; explains why warns were removed from the request hot path and validate/preparse paths.
---

# CORS Force-Wildcard (`**`) Does Not Emit `tracing::warn!` (Fixed)

## Conclusion

`Cors::from_arc` and `Cors::new` **must not** emit `tracing::warn!` for `allow_origins == "**"`, `allow_methods == "**"`, or `allow_headers == "**"`. The warning code was removed on 2026-05-16 because:

1. `Cors::from_arc` is called per CORS preflight request from `preflight_handler.rs:62` — emitting `tracing::warn!` there violates the project-wide data-plane hot-path prohibition (`skills/03-coding/observability/`).
2. `Cors::new` is called per `EdgionPlugins` preparse — moving the warn there would only downgrade per-request spam to per-reload spam without changing the underlying signal value.
3. The `**` literal is an explicit operator choice; the CRD schema doc on `CorsConfig.allow_origins` / `allow_methods` / `allow_headers` already states the security implication, and `CorsConfig::validation_error` hard-rejects the `wildcard + allow_credentials = true` combination. Runtime tracing adds no information the operator does not already have at configuration time.

The CORS plugin's own per-request observability is via `plugin_log.push("cors preflight" / "origin match:allow" / ...)` in `run_request` — that path is unchanged.

## Decision Rule

When a review reports patterns like:

1. "`Cors::new` / `Cors::from_arc` warns on every hot reload — should be moved to `validate`/`preparse`"
2. "Should add a `validation_warning` field to surface the `**` warning at first load only"
3. "Per-request preflight emits `tracing::warn!` when `allow_origins = '**'`"

→ close as **fixed + design intent**. Verify before closing:

- `Cors::from_arc` in `src/core/gateway/plugins/http/cors/plugin.rs` contains no `tracing::` calls.
- `Cors::new` contains no `tracing::` calls for the `**` cases.
- The CRD schema doc on `CorsConfig.allow_origins` / `allow_methods` / `allow_headers` still describes the `**` semantics.
- `CorsConfig::validation_error` still rejects `wildcard + allow_credentials = true`.

If a reviewer asks for a `validation_warning` infrastructure addition: not accepted — there is no `validation_warning` mechanism in `PluginValidation` today, and adding one to surface a single CORS warning is well above the cost of letting the CRD doc carry the message.

## Background Notes

- Decision date: 2026-05-16
- Source issue: `tasks/review-p3/P3-g4-008-cors-new-tracing-warn-on-every-reload.md` (closed)
- Related: `skills/03-coding/observability/00-access-log.md` (data-plane vs control-plane tracing scope)
