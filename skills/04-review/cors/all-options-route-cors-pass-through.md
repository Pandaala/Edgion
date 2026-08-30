---
name: all-options-route-cors-pass-through
description: Use when reviewing AllOptions preflight semantics; selecting every OPTIONS request does not make a route CORS filter own a request without Origin.
status: fixed
finding_id: change-fix-02
closed: 2026-08-02
---

# AllOptions Preserves Route CORS Ownership — FIXED

## Conclusion

`PreflightMode::AllOptions` widens global preflight selection to every `OPTIONS` request, but a
route-level CORS filter still owns only CORS requests. When an `OPTIONS` request has no `Origin`,
the route CORS filter passes it through to upstream; when no route CORS filter exists, the global
fallback replies locally with the configured `statusCode`.

Fix suggestions that force a local response solely because `AllOptions` selected the request are
**not accepted**.

## Core Rationale

**1. Selection and CORS ownership are separate decisions**

The global handler uses `AllOptions` to select the request before the route plugin pipeline. A
matched `EdgionPlugins` CORS configuration delegates processing to that pipeline, where the CORS
plugin returns `GoodNext` for a request without `Origin`. The same request is locally handled only
when no route CORS configuration exists and the global fallback is therefore the owner.

An `OPTIONS` request with `Origin` but without `Access-Control-Request-Method` follows the same
ownership split: the no-CORS fallback handles it locally in `AllOptions` mode, while a route CORS
filter treats it as an actual CORS request, forwards it, and adds response headers only when the
origin is allowed.

Because delegation uses the normal route plugin pipeline, entry-level CORS conditions also remain
authoritative. A non-matching condition skips the CORS plugin and leaves the request on the normal
upstream path.

**2. The boundary follows Envoy and Gateway API semantics**

Envoy's CORS filter continues without handling a request whose `Origin` is absent. Gateway API
GEP-1767 likewise defines a CORS request as one that includes `Origin`, and defines a standard
preflight as `OPTIONS` with both `Origin` and `Access-Control-Request-Method`. Edgion's
`AllOptions` mode is an extension around that CORS boundary, not a redefinition inside each route
CORS filter.

**3. Both 200 and 204 are valid successful preflight statuses**

Envoy returns `200 OK` for a locally handled preflight. Edgion defaults to `204 No Content`, and
Gateway API GEP-1767 explicitly permits either 200 or 204. Changing Edgion's default status is a
separate public-behavior decision and is not required to resolve the ownership documentation.

## Fix Suggestions Not Accepted

- Force a local `statusCode` response for no-`Origin` requests on routes with CORS — overrides
  the route CORS filter's non-CORS pass-through boundary.
- Change the default preflight status from 204 to 200 only for Envoy parity — both statuses are
  conformant, and the configured Edgion default is outside this documentation mismatch.
- Make `AllOptions` require `Origin` — collapses it into the standard detector and removes its
  no-CORS fallback use case for non-standard clients.

## Re-evaluation Triggers

Re-open this decision only if:

- `AllOptions` is explicitly redefined to force local responses even when a route CORS filter
  does not own the request.
- Route CORS ownership moves out of the normal route plugin pipeline.
- Gateway API changes the successful preflight status or CORS request requirements.

## Reference Cases

- Finding `change-fix-02` — documentation aligned 2026-08-02.
- [Envoy CORS filter](https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/cors_filter).
- [Gateway API GEP-1767](https://gateway-api.sigs.k8s.io/geps/gep-1767/).
- [preflight-route-cors-single-owner.md](preflight-route-cors-single-owner.md).
- `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs`.
- `edgion-gateway/src/plugins/http/cors/plugin.rs`.
- `docs/en/ops-guide/gateway/preflight-policy.md`.
