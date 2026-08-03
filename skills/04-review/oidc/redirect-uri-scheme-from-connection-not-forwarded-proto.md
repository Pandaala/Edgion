---
name: redirect-uri-scheme-from-connection-not-forwarded-proto
description: Use when reviewing OIDC redirect URI scheme derivation; path-form redirect targets take the scheme from the listener's real TLS state and never from X-Forwarded-Proto.
status: fixed
finding_id: review-change-fix-05
closed: 2026-08-02
---

# OIDC Path Redirect URIs Derive the Scheme From the Connection — FIXED

## Conclusion

`OpenidConnect::request_scheme` reads the downstream connection's real TLS state
(`PluginSession::downstream_is_tls`) and ignores `X-Forwarded-Proto` entirely. Path-form
`redirectUri` and `postLogoutRedirectUri` therefore advertise `https` to the IdP whenever the
request arrived over a TLS listener, and `https` again when the connection state is unavailable.

This is the scheme half of [redirect-uri-ignores-untrusted-forwarded-host.md](redirect-uri-ignores-untrusted-forwarded-host.md);
the two helpers share one threat model and must not diverge. Fix suggestions that restore
`X-Forwarded-Proto` precedence — including "restrict it to `http`/`https` and keep trusting it" —
are **not accepted**.

## Core Rationale

**1. The header is client-controlled on this path**

Nothing strips or validates `X-Forwarded-Proto` before plugin execution, and Edgion has no
trusted-proxy configuration for it. Trusting it let an anonymous request send
`X-Forwarded-Proto: http` and make the gateway advertise `http://<host>/callback` as the OAuth
`redirect_uri`. If the IdP has both schemes registered for the client — a common dev/prod parity
convention — the authorization code is delivered over plaintext; if only `https` is registered,
the login fails with `redirect_uri_mismatch`, a zero-cost DoS on login. The old code also returned
the header value verbatim (lowercased), so any token, not just `http`/`https`, reached the
`format!("{}://{}{}")` composition.

**2. The connection already knows the answer**

The downstream digest carries an `ssl_digest` only for a connection that completed a TLS handshake
on this listener — the same source the session-persistence cookie already uses for its `Secure`
decision. A missing digest is "unknown", not "plaintext", so `downstream_is_tls` returns `None`
there and `request_scheme` keeps `https` as the safe default.

**3. Absolute URLs cover front-terminated TLS**

A deployment where an edge LB terminates TLS and forwards plaintext to the gateway now derives
`http`. That is the same tradeoff already accepted for the host: both configuration fields accept
an absolute URL, which bypasses request-derived composition entirely. Adding OIDC-only proxy-trust
configuration for the scheme would duplicate a policy surface that does not exist anywhere else in
the gateway.

## Fix Suggestions Not Accepted

- Trust `X-Forwarded-Proto` by default — the original defect; an unauthenticated header steering an
  outbound security-relevant URL.
- Keep the header but restrict it to `http`/`https` — narrows the payload, not the trust boundary;
  the downgrade to `http` is exactly the value an attacker wants.
- Add an OIDC-only trusted-proxy flag for the scheme — duplicates proxy trust policy and expands the
  public configuration surface when absolute URLs already provide an explicit origin.
- Fall back to `http` when the TLS state is unavailable — inverts the safe default for a case that
  means "unknown".

## Not In Scope

The DSL `req.scheme` builtin (`plugins/http/dsl/lang/builtins.rs`) still reads `X-Forwarded-Proto`
and `X-Scheme`. It exposes a read-only request attribute to scripts and does not compose an
outbound security-relevant URL, so it keeps proxy-header semantics deliberately.

## Re-evaluation Triggers

Re-open this decision only if:

- The gateway introduces a canonical trusted forwarded-proto mechanism shared by every consumer.
- OIDC redirect generation receives a trusted, route-derived external origin distinct from the
  listener state.
- Absolute redirect URLs stop being supported.

## Reference Cases

- `review-change-fix-05` (fixed 2026-08-02), continuation of `gw-plugin-oidc-02`.
- `edgion-gateway/src/plugins/http/openid_connect/config_validation.rs` — `request_scheme`.
- `edgion-gateway/src/plugins/runtime/traits/session.rs` — `downstream_is_tls`.
- Regression tests `path_redirect_uris_ignore_x_forwarded_proto`,
  `path_redirect_uris_fall_back_to_https_when_tls_state_unknown`,
  `path_redirect_uris_use_http_on_a_plaintext_listener`.
- `docs/en/user-guide/edgion-plugins/openid-connect.md`.
