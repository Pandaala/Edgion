# gw-plug3-02: Plugins read only the `Host` header and miss the HTTP/2 `:authority`, breaking redirects, OIDC login, host-scoped access rules, and ForwardAuth context

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_redirect.rs:64-68,129` (`header_str("host")`, fallback `"localhost"`)
  - `edgion-gateway/src/plugins/http/openid_connect/config_validation.rs:25-33,48` (`request_host` → `MissingRedirectHost`); `openid_connect/logout.rs:31`
  - `edgion-gateway/src/plugins/http/request_access_policy/plugin.rs:336-340,234` (host-scoped URL candidates)
  - `edgion-gateway/src/plugins/http/forward_auth/plugin.rs:104` (`X-Forwarded-Host`)
  - `edgion-gateway/src/plugins/http/wasm/plugin.rs:200-203` (`request.host` property)
  - Correct canonical sources: `routes/http/proxy_http/pg_request_filter.rs:790-807` (`ctx.request_info.hostname` from `uri.host()` first), `PluginSession::get_request_hostname` (`session_adapter.rs:1232`), `PluginSession::request_uri_authority`; WAF already compensates (`plugins/http/waf/mod.rs:30-49`)
  - Pingora fork: `pingora-core/src/protocols/http/v2/server.rs:471-477` accepts `:authority` without `Host`; `pingora-proxy/src/proxy_h1.rs:123-168` synthesizes `Host` only when the **upstream** H1 request is built

## Summary
On HTTP/2 (and HTTP/3) the request authority travels in `:authority`. Browsers and most clients send no
`Host` header on HTTP/2. The Edgion Pingora fork keeps that authority in `req_header.uri` and does not
add a `Host` header to the downstream request. Pingora writes `Host` only later, when it builds an H1
upstream request. The route matcher handles this correctly through `ctx.request_info.hostname`.
Several plugins and filters, however, read `session.header_str("host")` directly, so for ordinary
HTTP/2 traffic they see no host at all.

## Current behavior (evidence)
- **RequestRedirect** (Gateway API core filter): `build_location` uses
  `config.hostname().or_else(|| original_host.map(host_without_port)).unwrap_or("localhost")`, where
  `original_host = session.header_str("host")`. A path- or scheme-only redirect (`hostname` unset,
  which is the common case) on an HTTPS listener therefore sends HTTP/2 clients
  `Location: https://localhost/...`. The route-level redirect in `routes/http/redirect_http.rs:205-209`
  checks the URI authority first and is correct, so the two redirect implementations disagree.
- **OpenID Connect**: with the default relative `redirectUri` (`/.edgion/oidc/callback`),
  `resolve_redirect_uri` calls `request_host(session)`. That returns `None` without a `Host` header,
  and the result is `AuthFailure::MissingRedirectHost` (HTTP 401, "Missing host header for redirect").
  Browser login through the authorization-code flow therefore fails for every HTTP/2 browser session
  unless the operator configures an absolute URL. `logout.rs:31` has the same dependency.
- **RequestAccessPolicy**: when any URL candidate has `hosts`, the host is
  `NormalizedHostname::new(session.header_str("host").unwrap_or_default())`, which is empty for HTTP/2.
  `!candidate.hosts.contains("")` makes every host-scoped candidate miss. RAP is an allow policy
  (`Matched` → allow, `NoMatch` → deny, `plugin.rs:522-528`), so every legitimate HTTP/2 request to a
  host-scoped URL allow entry is rejected, while the same request over HTTP/1.1 passes. The decision
  depends on the transport protocol instead of the target.
- **ForwardAuth**: `X-Forwarded-Host` is sent empty for HTTP/2. External authorizers such as
  Authelia, oauth2-proxy, and Pomerium select their policy by `X-Forwarded-Host`, so they deny the
  request or apply the wrong rule.
- **Wasm**: the Proxy-Wasm `request.host` property is empty for HTTP/2.
- The WAF adapter already knows about the gap and synthesizes `host` from
  `request_uri_authority()` for HTTP/2 (`waf/mod.rs:30-49`). The fix exists locally but is not shared.

## Industry comparison
- **Envoy** normalizes HTTP/1 `Host` and HTTP/2 `:authority` into one `:authority` header before any
  filter runs. Filters therefore see a single host field (`Http::Headers::get().Host` is
  `:authority`).
- **NGINX** exposes `$host` / `$http_host` from either `Host` or `:authority`. The `auth_request` and
  `return 301 https://$host$request_uri` idioms work unchanged over HTTP/2.
- **Kong / APISIX** expose `ngx.var.host` / `kong.request.get_host()`, which come from the same
  normalized source.

## Impact
- A standard Gateway API filter (RequestRedirect) emits wrong `Location` values for most browser
  traffic on HTTPS listeners.
- The built-in OIDC login flow fails for HTTP/2 browsers in its default configuration.
- Host-scoped RequestAccessPolicy allow entries reject all HTTP/2 traffic. This is an availability
  failure that HTTP/1.1 tests do not catch. It would become a bypass for any future host-scoped deny
  semantics that copy the same pattern.
- External authorization decisions lose their host context.

## Suggested direction
1. Add one canonical accessor (for example `PluginSession::request_authority()` returning the raw
   authority with port). It reads `uri.authority()` first and then the single `Host` value, mirroring
   `pg_request_filter`. Route every plugin through it, and forbid direct `header_str("host")` in
   plugins with a lint or grep guard. Use the existing frozen `get_request_hostname()` wherever only
   the normalized hostname is needed (RAP, Wasm).
   - Alternative: normalize once at request entry by inserting `Host` from `:authority` for HTTP/2 and
     HTTP/3 (as Envoy does conceptually). Check the effect on upstream HTTP/2 framing first, because
     the fork's `proxy_h2.rs:259-288` has explicit Host/authority handling.
2. Remove the `"localhost"` fallback in RequestRedirect. With no authority, fail with the existing
   500 path rather than redirecting to a wrong host.
3. Add HTTP/2 integration cases for RequestRedirect without `hostname`, the OIDC default redirect URI,
   and a host-scoped RAP rule.

## Notes
- Searched `skills/04-review` for `:authority`, `request_uri_authority`, and `host-header`.
  `host-header/regression-guards/grpc-empty-authority.md` and
  `redirect/regression-guards/host-split-ipv6-literal.md` cover other aspects (empty authority on gRPC,
  IPv6 brackets). Neither establishes `Host` as the intended plugin source, and no page accepts this
  behavior.
- `uri-normalization/mandatory-design/normalization-contract.md` covers path and query only.
- No overlapping item exists under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED (P1)
- `plugins/runtime/gateway_api_filters/request_redirect.rs:129` reads only `session.header_str("host")`; `:67-68` falls back to `"localhost"`.
- `routes/http/proxy_http/pg_request_filter.rs:790-805` (route hostname) deliberately prefers `req_header.uri.host()` for HTTP/2 before `Host`, confirming the authority lives in the URI for HTTP/2 and that plugins reading only `Host` diverge from routing.
- The ACME challenge lookup at `pg_request_filter.rs:955-968` also reads only `Host` (HTTP-01 is normally HTTP/1.1 on port 80, so low impact, but the same helper should be used).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `request_redirect.rs`, `openid_connect/config_validation.rs`, `openid_connect/logout.rs`, `request_access_policy/plugin.rs`, `forward_auth/plugin.rs` and `wasm/plugin.rs` have no diff.
- Other `openid_connect/` files changed (session cookie, callback, token exchange) but none switches host derivation to `get_request_hostname`/`:authority`.
