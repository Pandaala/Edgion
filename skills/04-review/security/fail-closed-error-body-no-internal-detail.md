---
name: fail-closed-error-body-no-internal-detail
description: When reviewing data-plane plugin fail-closed responses (HTTP 500 / status code via `PluginRunningResult::ErrResponse`), the response body must not leak internal plugin type names, internal failure mode strings, or other gateway-architecture detail. Operator-actionable detail belongs in `PluginLog::push_err`, not in the client-visible body.
---

# Fail-closed error body must not leak internal plugin/failure detail

## Conclusion

For data-plane plugins that fail-close with `PluginRunningResult::ErrResponse { status, body }`,
the `body` (if present) must be a generic client-facing message. It must not embed:

- The internal plugin type name (e.g. `region-route`, plugin enum variant names).
- The internal failure mode string (e.g. `reference unresolved`, `BrokenRef`, internal state tag).
- Any other detail that lets a downstream client or scanner reverse-engineer the gateway's
  internal plugin architecture.

The operator-actionable diagnostic must already be on `PluginLog::push_err(...)` in the plugin
(it surfaces in the access log); duplicating that string in the client-visible body buys nothing
for the operator and continuously leaks internal topology to every request that hits the
misconfigured route.

`body: None` is also acceptable (e.g. `extension_ref::run_extension`). A short, generic
`{"message":"Internal gateway error"}` is acceptable when a JSON body is expected. Prefer the
shortest body that conveys "the gateway has an internal problem" without naming the subsystem.

## Core rationale

- The original framing — "leak the plugin type so the operator can grep the access log" — is
  upside down: the access log already has the precise tag from `push_err`, and only operators
  with log access can act on it. The client-visible body reaches every counterparty.
- Information-disclosure findings against fail-closed bodies are easy and routine in audits.
  Even when severity is Low, leakage is *continuous* for every request to the misconfigured
  route, so the cost/benefit of a 1-line rewrite is favorable.
- The rule does **not** apply to client-actionable validation errors that are part of the
  documented plugin contract (e.g. `direct_endpoint` `"Missing target endpoint"`,
  `dynamic_internal_upstream` `"Missing routing key for backend selection"`). Those describe a
  client-side prerequisite the operator told the API consumer about; they are not a leak of
  gateway-internal topology and are out of scope for this rule.

## Fix suggestions not accepted

- "Add the namespace/resource name to the body for easier operator triage" — operator triage
  is via access log; the body must remain free of internal identifiers.
- "Echo the internal failure-mode tag (`BrokenRef`, internal enum variant name) so support
  engineers can correlate" — same path; support correlates via log, not body.
- "Add a debug-mode toggle that includes detail in the body" — a runtime branch on the
  data-plane fail-closed path is unjustified; toggle bugs become production leaks.

## Re-evaluation triggers

- A new fail-closed code path lands in a plugin and embeds an internal plugin/failure-mode
  string in the response body → flag it.
- A future feature genuinely needs a structured client-facing error contract (e.g. RFC 9457
  `application/problem+json`) — at that point a richer body becomes possible, but the
  internal-name/failure-mode prohibition still stands; only documented public fields are
  client-visible.

## Reference cases

- **plugins-http-F016 (2026-05-27)**: `region_route` BrokenRef body was
  `{"message":"region-route reference unresolved"}` → replaced with
  `{"message":"Internal gateway error"}`. The internal `push_err` tags
  (`service region-route ref unresolved`, `service region-route missing clusterRef`,
  `cluster region-route ref unresolved`) remain in `resolve_config` for the access log.
  See `src/core/gateway/plugins/http/region_route/plugin.rs` BrokenRef arm.
- **gateway_api_filters::extension_ref (existing pattern)**: uses `body: None` on its
  fail-closed 500 — the canonical "no detail leak" shape; safe template.
- **Sibling plugins with client-actionable validation messages**
  (`direct_endpoint`, `dynamic_internal_upstream`, `dynamic_external_upstream`): not covered
  by this rule (see "Core rationale" — client-facing prerequisites, not internal topology).
