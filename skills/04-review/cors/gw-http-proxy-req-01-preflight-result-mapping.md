---
name: preflight-result-mapping
description: Use when reviewing the global CORS preflight handler's response ownership; documents that pass-through plugin results must continue the proxy pipeline instead of being treated as an already-written response.
status: fixed
finding_id: gw-http-proxy-req-01
closed: 2026-08-02
---

# Global CORS Preflight Result Mapping — FIXED

## Conclusion

The original fix established that pass-through CORS results must continue the normal proxy
pipeline. The follow-up `change-fix-01` removed the ad-hoc CORS execution from
`PreflightHandler`: a route CORS configuration now delegates directly to the normal route
plugin pipeline, which owns both continuation and terminal responses without double execution.

Fix suggestions that keep returning `Ok(true)` whenever `cors_config` is present are **not
accepted**.

## Core Rationale

**1. Pingora's boolean is response ownership**

`pg_request_filter::request_filter` returns the boolean from `PreflightHandler` directly to
Pingora. Pingora exits the request when it receives `Ok(true)` and continues to plugin and
upstream processing only for `Ok(false)`. A route CORS configuration therefore returns
`Ok(false)` without being run ad hoc; the normal route runtime decides the final outcome.

**2. CORS has legitimate pass-through results**

`Cors::run_request` returns `GoodNext` when the request has no `Origin` header. It also returns
`GoodNext` for an allowed preflight with `preflight_continue: true`; that path queues response
headers for the upstream response phase and deliberately does not write a local response. In
`PreflightMode::AllOptions`, an OPTIONS request without `Origin` also reaches the normal route
pipeline. Running these cases first through an ad-hoc instance would duplicate queued headers.

**3. Regression coverage is at the handler boundary**

`preflight_handler.rs` tests drive a real Pingora HTTP/1.1 `Session`. The continuation test
verifies that the handler delegates without queueing headers, then runs the route CORS instance
once and observes exactly one `Vary: Origin`; `AllOptions` without `Origin` also continues.

## Fix Suggestions Not Accepted

- Always return `Ok(true)` when a CORS config exists — closes the connection without a response
  for pass-through cases.
- Run an ad-hoc CORS instance before the normal route runtime — duplicates pass-through header
  operations and bypasses normal runtime ownership.
- Move the behavior change into `Cors::run_request` by terminating `preflight_continue` requests
  — contradicts the documented purpose of `preflight_continue`.
- Treat `AllOptions` as requiring an `Origin` header — changes the explicit policy semantics and
  leaves non-browser OPTIONS requests unsupported.

## Re-evaluation Triggers

Re-open this decision only if:

- The normal route plugin runtime stops fully handling a `PluginRunningResult` variant before
  returning control to Pingora.
- The route CORS plugin stops running in the normal request-plugin pipeline.
- `preflight_continue` semantics change from upstream forwarding to local response handling.

## Reference Cases

- Finding `gw-http-proxy-req-01` — fixed 2026-08-02.
- Follow-up `change-fix-01` — single route-runtime ownership fixed 2026-08-02.
- Follow-up `change-fix-03` — obsolete handler-result mapping finding closed 2026-08-02.
- [preflight-route-cors-single-owner.md](preflight-route-cors-single-owner.md).
- Follow-up `change-fix-02` — `AllOptions` ownership and documentation aligned 2026-08-02.
- [all-options-route-cors-pass-through.md](all-options-route-cors-pass-through.md).
- `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs`.
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`.
- `docs/en/user-guide/http-route/filters/edgion-plugins/cors.md`.
- `docs/en/ops-guide/gateway/preflight-policy.md`.
