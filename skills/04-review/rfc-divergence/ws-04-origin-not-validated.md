---
name: ws-04-origin-not-validated
description: Accepted RFC 6455 §10.2/§4.2.2 divergence — Edgion does not validate the WebSocket handshake Origin and ships no gateway-layer WS origin allowlist; CSWSH defense is the backend's job. Reject re-reports proposing a gateway-side allowedOrigins field.
---

# ws-04 — WebSocket `Origin` not validated at the gateway

**Status:** ⚠️ accepted divergence · 2026-05-20
**RFC:** RFC 6455 §10.2, §4.2.2 (SHOULD / MAY)

## Conclusion

Edgion forwards the WebSocket handshake transparently and does not inspect the `Origin` header; there is no per-route WS origin allowlist. Cross-Site WebSocket Hijacking (CSWSH, CWE-1385) defense is delegated to the backend.

Suggestions of the form "add `allowedOrigins` to the route config / a dedicated WS plugin, return 403 on mismatch, default any-origin with a startup warn" are **not accepted** as a gateway feature.

## RFC clause

> "Servers that are not intended to process input from any web page but only for certain sites SHOULD verify the |Origin| field … SHOULD respond … with a reply containing HTTP 403 Forbidden status code." — §10.2

> "the server MAY reject the connection based on the value of the |Origin| header field." — §4.2.2

## Code shape today

- `routes/http/proxy_http/pg_request_filter.rs:256-261` — WS is tagged from the `Upgrade` header only; no `Origin` read on the WS path. The upgrade is forwarded later in the upstream phase.
- `plugins/runtime/gateway_api_filters/cors_filter.rs` (`GapiCorsFilter`, GEP-1767) and `plugins/http/cors/plugin.rs` (`Cors`) — both check `Origin` for HTTP CORS but return `GoodNext` (pass-through) on a non-preflight mismatch by design; neither gates the handshake. Browsers do not apply CORS to WebSocket anyway, so CORS response headers give zero CSWSH protection.

## Why we diverge

1. **RFC strictness is SHOULD/MAY, not MUST.** Project policy fixes SHOULD/MAY only when zero design conflict exists; adding a config-plane field + data-plane reject is a design action, not a mechanical patch.
2. **Industry norm is to delegate to the backend.** No major L7 gateway/proxy ships first-class WS origin validation — Envoy, NGINX, Traefik, Caddy, HAProxy, Kong, APISIX all delegate to the app or to generic RBAC/ACL/ext_authz. First-class support lives in application frameworks (ASP.NET `WebSocketOptions.AllowedOrigins`, Spring `setAllowedOrigins`). Gateway API has no standard field or GEP for it.
3. **Narrow exploitability.** CSWSH only bites WS backends authenticated by cookies / ambient credentials; Bearer / custom-header auth is immune (a cross-origin browser WS cannot set those headers). The backend knows its own auth model and allowed origins, so the check belongs closest to it.
4. **WS is a transparent tunnel by design.** Operators who genuinely need gateway-layer enforcement have an escape hatch via the plugin/DSL system (the Edgion analogue of Envoy RBAC / Istio `AuthorizationPolicy`).

## Re-evaluation triggers

- Gateway API introduces a standard WS-origin field / GEP.
- An operator-reported CSWSH incident through Edgion against a cookie-auth WS backend that cannot self-remediate.
- Edgion pivots from transparent WS tunnel to a terminating WS server.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a3.md → V4` (CONFIRM, SHOULD/MAY qualification)
- Industry research (2026-05-20): L7 gateways delegate to backend; ASP.NET / Spring first-class; Gateway API has no GEP. OWASP WebSocket Security Cheat Sheet; CWE-1385 Missing Origin Validation in WebSockets.
- Closure: [CLOSED-FINDINGS.md → ws-04](../CLOSED-FINDINGS.md)
