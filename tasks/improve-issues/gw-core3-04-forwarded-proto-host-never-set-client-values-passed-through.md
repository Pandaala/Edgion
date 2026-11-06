# gw-core3-04: The proxy never sets X-Forwarded-Proto/Host/Port and forwards client-supplied values verbatim

- **Category**: best-practice (with a security consequence)
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs:62-67`, `:121-138` (`finalize_forwarded_headers`: only `X-Forwarded-For` and the RemoteIp header are owned)
  - `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:62-` (`snapshot_x_forwarded_for` is the only forwarded-header normalization)
  - No producer anywhere in `edgion-gateway/src` (grep for `x-forwarded-proto|x-forwarded-host|x-forwarded-port|forwarded`: only the DSL `req.scheme` reader, OIDC comments and ForwardAuth's outbound sub-request)
  - Related evidence: `skills/04-review/oidc/regression-guards/redirect-uri-scheme-from-connection-not-forwarded-proto.md:28-30` ("Nothing strips or validates `X-Forwarded-Proto` before plugin execution")

## Summary
After all request plugins, Edgion publishes exactly two proxy-owned identity headers upstream:
`X-Forwarded-For` (appended) and the configured RemoteIp header (default `X-Real-IP`). It never emits
`X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Port` or RFC 7239 `Forwarded`, and it does not strip
or overwrite those headers when a client sends them. Backends behind an HTTPS listener therefore believe the
request was plain HTTP, and any client on a plain-HTTP listener (or any client at all) can assert
`X-Forwarded-Proto: https` / `X-Forwarded-Host: victim.example` and have the gateway relay it as if it were
gateway-attested.

## Current behavior (evidence)
- `upstream_request_filter` ends with `finalize_forwarded_headers(&ctx.egc.remote_ip_header, ...)`
  (`pg_upstream_request_filter.rs:67`). That function writes `X-Forwarded-For = <inbound XFF>, <peer>` and
  `<remote_ip_header> = remote_ip` (`:121-138`); nothing else.
- The inbound request header map is otherwise forwarded unchanged, so client-supplied
  `X-Forwarded-Proto`, `X-Forwarded-Host`, `X-Forwarded-Port`, `X-Forwarded-Prefix` and `Forwarded` reach
  the backend verbatim.
- The project already treats these headers as attacker-controlled inside the gateway: the OIDC plugin was
  changed to ignore `X-Forwarded-Proto`/`-Host` because "Nothing strips or validates `X-Forwarded-Proto`
  before plugin execution, and Edgion has no trusted-proxy configuration for it" (review page cited above),
  and the CSRF plugin refuses to derive `Secure` from it. The same untrusted value is nevertheless handed to
  every upstream application.

## Industry comparison
- Envoy (edge, `use_remote_address: true`): always sets `x-forwarded-proto` from the downstream connection,
  overwriting any client value; `x-forwarded-host`/`-port` are appended when `append_x_forwarded_host` /
  `append_x_forwarded_port` are enabled; trust of inbound values is governed by `xff_num_trusted_hops`.
- ingress-nginx: sets `X-Forwarded-Proto/Host/Port/Scheme` from `$pass_access_scheme`, honouring inbound
  values only with `use-forwarded-headers`/trusted `proxy-real-ip-cidr`.
- Traefik: sets `X-Forwarded-Proto/Host/Port/Server` and strips untrusted inbound values unless
  `forwardedHeaders.trustedIPs` contains the peer.
- Kong / APISIX: set `X-Forwarded-Proto/Host/Port/Path/Prefix`, trusting inbound values only from
  `trusted_ips`.

## Impact
- Correctness: frameworks (Spring `ForwardedHeaderFilter`, Django `SECURE_PROXY_SSL_HEADER`, Rails,
  Express `trust proxy`, ASP.NET `ForwardedHeaders`) generate `http://` absolute URLs, OAuth redirect URIs
  and links, or loop forever on "redirect to HTTPS" behind an Edgion HTTPS listener. Each team must hand-add
  a header filter per route, and no template variable for the real scheme exists for them to use.
- Security: on any listener, a client controls the scheme/host the backend trusts. Typical consequences are
  bypassing backend HTTPS-only enforcement (`SECURE_SSL_REDIRECT`, HSTS issuance), host-header poisoning of
  password-reset links and cache keys via `X-Forwarded-Host`, and per-tenant routing confusion in apps that
  key on forwarded host.

## Suggested direction
- Extend `finalize_forwarded_headers` (the single proxy-owned identity publication point) to set
  `X-Forwarded-Proto` from the downstream TLS state, `X-Forwarded-Host` from the normalized request
  authority and `X-Forwarded-Port` from the listener port, overwriting inbound values by default.
- Reuse the existing RealIp trust decision (trusted-proxy IpLists already captured per request) to
  optionally preserve inbound `X-Forwarded-Proto/Host/Port` only when the direct peer is trusted, mirroring
  Envoy `xff_num_trusted_hops` / Traefik `trustedIPs`; strip them otherwise.
- Keep it EGC-level (one policy, resolved at publication) rather than a per-route plugin, and document the
  contract next to `forwardedHeaders.remoteIpHeader`.

## Notes
- Checked `skills/04-review/oidc/regression-guards/redirect-uri-scheme-from-connection-not-forwarded-proto.md`,
  `redirect-uri-ignores-untrusted-forwarded-host.md`, `skills/04-review/csrf/regression-guards/cookie-secure-configurable.md`,
  `skills/04-review/real-ip/` and `skills/04-review/host-header/`. They only decide how *plugins* treat these
  headers; none decides what the proxy publishes upstream, and none records an intentional "do not set
  X-Forwarded-Proto" decision. No existing `tasks/` entry.

## Verification (independent)

Verdict: CONFIRMED. Severity P1 kept.
- `finalize_forwarded_headers` (`pg_upstream_request_filter.rs:121-138`) writes only `X-Forwarded-For` and the RemoteIp header.
- Grep over `edgion-gateway/src` and `edgion-resources/src` finds no producer or stripper of `X-Forwarded-Proto/Host/Port`/`Forwarded`; only ForwardAuth's sub-request header, a CredentialInjector deny list, and the DSL `req.scheme` reader that itself trusts the client `X-Forwarded-Proto` (`dsl/lang/builtins.rs:685`).
- The OIDC review page already treats these headers as untrusted inside the gateway; nothing addresses what is published upstream. Table-stakes behavior in Envoy/ingress-nginx/Traefik/Kong.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs` has no diff; `finalize_forwarded_headers` still owns only XFF and the RemoteIp header.
- Grep for `x-forwarded-proto` in `edgion-gateway/src` still hits only `openid_connect/config_validation.rs` and `dsl/lang/builtins.rs`.
