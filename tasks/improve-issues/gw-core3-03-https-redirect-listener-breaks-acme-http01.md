# gw-core3-03: An HTTP listener with HTTP→HTTPS redirect can never serve built-in ACME HTTP-01 challenges

- **Category**: design-error
- **Severity**: P1 (significant; built-in certificate issuance/renewal silently fails in the most common deployment)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/runtime/server/listener_builder.rs:195-222` (a Redirect declaration installs `EdgionHttpRedirectProxy` instead of `EdgionHttpProxy`)
  - `edgion-gateway/src/routes/http/redirect_http.rs:184-254` (`request_filter` 301s every request; no challenge lookup)
  - `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:166-168`, `:917-954` (`try_serve_acme_challenge` exists only in the main proxy)
  - `edgion-gateway/src/services/acme/challenge_store.rs:101`, `:385-401` (challenge attachments require protocol `HTTP` and an exact HTTP listener key, so the redirected HTTPS request cannot match)
  - Controller: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs:55-131` (`resolve_acme_listener_attachments` accepts any `protocol == "HTTP"` listener, including redirect-annotated Gateways, and reports success)

## Summary
`edgion.io/http-to-https-redirect: "true"` turns the Gateway's HTTP listeners into a separate
`EdgionHttpRedirectProxy` that answers every request with a 301 to HTTPS. That proxy never consults the
ACME challenge store. The 301 target (the HTTPS listener) also cannot answer, because challenge
attachments are bound to the HTTP listener key and protocol. The Controller nevertheless accepts the
EdgionAcme `gatewayRef` and schedules the order. Result: HTTP-01 validation always fails for Gateways that
redirect HTTP to HTTPS, which is the default production setup for public sites, and renewals fail the same
way after the redirect is turned on.

## Current behavior (evidence)
- `add_http_listener` checks `HttpServiceDeclaration::from_annotation_lookup`; for `Redirect` it builds an
  `EdgionHttpRedirectProxy` service and returns early (`listener_builder.rs:195-222`). No route manager,
  no `EdgionHttpProxy` hooks.
- `EdgionHttpRedirectProxy::request_filter` validates Host, checks hostname admission, and writes a 301 to
  `https://host[:port]/path?query` for every path (`redirect_http.rs:184-254`), including
  `/.well-known/acme-challenge/<token>`.
- The only challenge responder is `try_serve_acme_challenge` in the main proxy's `request_filter`
  (`pg_request_filter.rs:166`, `:917-954`). On the HTTPS listener it calls
  `lookup_for_listener(&edgion_http.listener_key, ...)`, but `attachment_matches` requires
  `listener_key.protocol() == "HTTP"` and equality with the attachment's HTTP listener key
  (`challenge_store.rs:385-401`), so the lookup fails and the request falls through to normal routing
  (typically 404).
- The Controller resolves EdgionAcme attachments by selecting `listener.protocol == "HTTP"` listeners
  without looking at the redirect annotation (`route_utils.rs:115-131`); the EdgionAcme is reported as
  resolved and the ACME order proceeds to a validation that cannot succeed.
- `docs/en/ops-guide/gateway/http-to-https-redirect.md` and `docs/en/ops-guide/gateway/tls/acme.md` do not
  mention the incompatibility.

## Industry comparison
- cert-manager + ingress-nginx: the solver ingress/route for `/.well-known/acme-challenge/` is exempted from
  `ssl-redirect`; Gateway API HTTP-01 solvers attach an HTTPRoute that takes precedence over redirect routes.
- Traefik: the ACME HTTP challenge router has the highest priority on the entrypoint and is served before
  any `redirectScheme` middleware.
- Caddy and NGINX (`location ^~ /.well-known/acme-challenge/` before `return 301`) follow the same rule:
  the challenge path is always answered over plain HTTP.

## Impact
- Built-in ACME cannot issue or renew certificates for any Gateway that redirects HTTP to HTTPS; renewals
  start failing as soon as an operator enables the redirect after first issuance, and certificates expire
  (compounded by `ctrl-svc-02`, where renewal eventually stops retrying).
- Operators get a green EdgionAcme attachment and a failing CA validation with no gateway-side signal.

## Suggested direction
1. In `EdgionHttpRedirectProxy::request_filter`, before the Host/redirect logic, call the same
   `try_serve_acme_challenge` lookup (challenge store + this listener's key and live `GatewayInfoIndex`)
   and only redirect when no active challenge matches. This is the standard "challenge path wins over
   redirect" rule and keeps the redirect proxy small.
2. Optionally also let the HTTPS listener answer a challenge whose attachment names the same Gateway, so
   CAs that follow the redirect still validate (Let's Encrypt follows redirects to 443 and ignores the
   certificate).
3. Until fixed, have the Controller report `Accepted=False`/a clear condition reason on EdgionAcme when the
   referenced HTTP listener is a redirect service, and document the limitation.

## Notes
- Checked `skills/04-review/acme/` (intentional-design `acme-service-shape.md`, regression guards
  `acme-http01-gatewayref-serving-ownership.md`, `challenge-host-split-ipv6-literal.md`) and
  `skills/04-review/redirect/`: none covers the redirect listener interaction. Not covered by `ctrl-svc-02/03/04`
  (renewal retry, propagation wait, renewal window).

## Verification (independent)

Verdict: CONFIRMED. Severity P1 kept.
- A Redirect declaration installs `EdgionHttpRedirectProxy` and returns before the normal proxy is built (`listener_builder.rs:195-222`); `redirect_http.rs` contains no ACME/challenge reference (grep).
- `attachment_matches` (`edgion-gateway/src/services/acme/challenge_store.rs:385-401`) requires `listener_key.protocol() == "HTTP"` and the exact HTTP listener key, so the HTTPS target of the 301 cannot answer.
- Neither `docs/en/ops-guide/gateway/tls/acme.md`, `http-to-https-redirect.md` nor `skills/02-features/03-resources/17-acme.md` mentions the incompatibility; no `skills/04-review/acme` page covers it.
- Kept at P1: the redirect annotation plus built-in ACME is the natural production combination, and the end state is certificate expiry (outage). A workaround exists (Gateway API `RequestRedirect` HTTPRoute on a normal HTTP listener, where `try_serve_acme_challenge` runs first) but is undocumented.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `edgion-gateway/src/runtime/server/listener_builder.rs`, `routes/http/redirect_http.rs` and `services/acme/` have no diff.
- Controller `route_utils.rs` unchanged; ACME docs still do not mention the incompatibility.
