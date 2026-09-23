---
name: send-error-response-header-insert-plugin-log
description: Use when reviewing forward_auth::send_error_response findings of the form "let _ = resp.insert_header(...) swallows errors / invalid headers from auth service leave no PluginLog trace".
knowledge_type: regression-guards
---

# forward_auth::send_error_response Swallows insert_header Errors (P3-g3-016)

**Status / verdict**: fixed (P3-g3-016)

When reviewing `forward_auth::send_error_response` "`let _ = resp.insert_header(...)` swallows errors / invalid headers from auth service leave no PluginLog trace" findings: **fixed (P3-g3-016)**.

`send_error_response` takes `plugin_log: &mut PluginLog` and records fixed
`invalid response header` diagnostics on failed selected-header writes. Selected
names now validate at construction; selected text values preserve allowed
repeated field lines, while non-text values remain excluded. Shared response protection and received Connection
nomination filtering happen before copying. This supersedes the historical
`to_str()`-filtered extra-header description. Static framing writes belong to
this response's body/transport owner; write failures do not justify new tracing.

## Source paths

- `edgion-gateway/src/plugins/http/forward_auth/plugin.rs` — `send_error_response`

`ForwardAuthDecision.client_headers` selects auth response fields to copy.
`send_error_response` receives the source response map and applies the shared
policy before inserting/appending selected values. See
[denial header preservation](deny-response-content-type-preserved.md).

## Re-report guard

If a finding claims the `extra_headers` insert silently drops, verify the current `edgion-gateway/src/plugins/http/forward_auth/plugin.rs::send_error_response` before re-reporting.

## Generalised rule

Data-plane error-response helpers that echo operator-configured header names should funnel insert failures through PluginLog, not `tracing`.

## Resolution references

- `P3-g3-016` — —.
