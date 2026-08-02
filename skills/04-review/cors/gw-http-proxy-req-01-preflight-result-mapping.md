---
name: preflight-result-mapping
description: Use when reviewing the global CORS preflight handler's response ownership; documents that pass-through plugin results must continue the proxy pipeline instead of being treated as an already-written response.
status: fixed
finding_id: gw-http-proxy-req-01
closed: 2026-08-02
---

# Global CORS Preflight Result Mapping — FIXED

## Conclusion

`PreflightHandler::handle_preflight` must preserve the `PluginRunningResult` contract when it
delegates to `Cors::run_request`: `GoodNext` and `Nothing` continue the normal proxy pipeline,
while a terminal result ends the request after the CORS plugin has handled the response. The
handler must not infer response ownership from the presence of a CORS configuration.

Fix suggestions that keep returning `Ok(true)` whenever `cors_config` is present are **not
accepted**.

## Core Rationale

**1. Pingora's boolean is response ownership**

`pg_request_filter::request_filter` returns the boolean from `PreflightHandler` directly to
Pingora. Pingora exits the request when it receives `Ok(true)` and continues to upstream
processing only for `Ok(false)`. Therefore the boolean must describe whether a response was
handled, not whether a plugin configuration was found.

**2. CORS has legitimate pass-through results**

`Cors::run_request` returns `GoodNext` when the request has no `Origin` header. It also returns
`GoodNext` for an allowed preflight with `preflight_continue: true`; that path queues response
headers for the upstream response phase and deliberately does not write a local response. In
`PreflightMode::AllOptions`, an OPTIONS request without `Origin` reaches the handler even though
the CORS plugin skips processing. Both cases must reach the ordinary proxy pipeline.

**3. Regression coverage is at the handler boundary**

`preflight_handler.rs` tests drive a real Pingora HTTP/1.1 `Session` and assert that both
`preflight_continue` and `AllOptions` without `Origin` return `response_sent == false`. The
standard CORS preflight detection test also uses the real session path instead of a placeholder
request header.

## Fix Suggestions Not Accepted

- Always return `Ok(true)` when a CORS config exists — closes the connection without a response
  for pass-through cases.
- Always return `Ok(false)` for a CORS config — forwards preflights that already wrote the local
  204 response and can cause a second response attempt.
- Move the behavior change into `Cors::run_request` by terminating `preflight_continue` requests
  — contradicts the documented purpose of `preflight_continue`.
- Treat `AllOptions` as requiring an `Origin` header — changes the explicit policy semantics and
  leaves non-browser OPTIONS requests unsupported.

## Re-evaluation Triggers

Re-open this decision only if:

- `PluginRunningResult` gains new continuation or terminal variants and this mapping is not
  updated.
- The preflight handler stops delegating directly to `Cors::run_request` or its caller no longer
  passes the boolean directly to Pingora.
- `preflight_continue` semantics change from upstream forwarding to local response handling.

## Reference Cases

- Finding `gw-http-proxy-req-01` — fixed 2026-08-02.
- `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs`.
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`.
- `docs/en/user-guide/http-route/filters/edgion-plugins/cors.md`.
- `docs/en/ops-guide/gateway/preflight-policy.md`.
