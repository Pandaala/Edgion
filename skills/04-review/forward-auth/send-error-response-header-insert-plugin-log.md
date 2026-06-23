---
name: send-error-response-header-insert-plugin-log
description: Use when reviewing forward_auth::send_error_response findings of the form "let _ = resp.insert_header(...) swallows errors / invalid headers from auth service leave no PluginLog trace".
---

# forward_auth::send_error_response Swallows insert_header Errors (P3-g3-016)

**Status / verdict**: fixed (P3-g3-016)

When reviewing `forward_auth::send_error_response` "`let _ = resp.insert_header(...)` swallows errors / invalid headers from auth service leave no PluginLog trace" findings: **fixed (P3-g3-016)**.

`send_error_response` takes `plugin_log: &mut PluginLog` and emits `plugin_log.push_err("invalid response header")` when an `extra_headers` entry from the configured `decision.client_headers` fails `insert_header`. The static `Content-Type` / `Connection` inserts and the `write_response_*` calls retain `let _ = ...` intentionally — the static headers cannot fail, and `write_response_*` failures mean the downstream connection is already closed (no actionable observability). The `extra_headers` values are pre-filtered by `HeaderValue::to_str()` at the call site (operator-configured `decision.client_headers` echo from auth service response), so realistic failures are operator misconfiguration with a non-token byte in a header **name**.

## Source paths

- `edgion-gateway/src/plugins/http/forward_auth/plugin.rs` — `send_error_response`

The `client_headers` that feed `extra_headers` are now located at `decision.client_headers` in the config struct (`ForwardAuthDecision.client_headers: Vec<String>`) rather than the old top-level `clientHeaders` field. The call path is unchanged: the plugin runtime collects matching headers from the auth response, passes them as `Some(&extra)` to `send_error_response`, and `send_error_response` calls `plugin_log.push_err` on insert failure.

## Re-report guard

If a finding claims the `extra_headers` insert silently drops, verify the current `edgion-gateway/src/plugins/http/forward_auth/plugin.rs::send_error_response` before re-reporting.

## Generalised rule

Data-plane error-response helpers that echo operator-configured header names should funnel insert failures through PluginLog, not `tracing`.
