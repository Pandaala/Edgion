---
name: csrf-cookie-secure-configurable
description: Use when reviewing CSRF plugin cookie findings; covers the configurable `cookieSecure` attribute and why a hardcoded `Secure=true` re-report is stale.
---

# CSRF cookie `Secure` is operator-configurable — FIXED

## Conclusion

The CSRF token cookie's `Secure` attribute is driven by `CsrfConfig.cookie_secure` (default `true`), not hardcoded. Re-reports of the form "`csrf/plugin.rs` hardcodes `.secure(true)`, breaks HTTP-only deployments (RFC 6265bis §4.1.2.5 / §5.2.5)" are **stale** — closed as cookie-auth-04.

Fix suggestions of the form "auto-derive `Secure` from `x-forwarded-proto`" are **not accepted**.

## Core Rationale

**1. Configurable, default-secure**

`set_csrf_cookie` builds the cookie with `.secure(self.config.cookie_secure)` (`edgion-gateway/src/plugins/http/csrf/plugin.rs`). The default is `true`, so the secure-by-default behavior is preserved; operators terminating TLS upstream (LB/sidecar) or running mesh-internal plain HTTP set `cookieSecure: false`.

**2. Plain `bool`, consistent with OIDC**

The chosen shape mirrors the existing OIDC `session_cookie_secure: bool` precedent (`edgion-resources/src/resources/edgion_plugins/plugin_configs/openid_connect.rs`). A plain operator-set flag is explicit and does not trust the client-suppliable `x-forwarded-proto` header.

## Fix Suggestions Not Accepted

- `cookie_secure: Option<bool>` + auto-derive from `x-forwarded-proto` — trusts a client-controllable header, diverges from the OIDC config precedent, and adds complexity for no real benefit (the breaking case still requires explicit operator intent).
- Removing the `Secure` attribute entirely to "fix HTTP" — drops a security default for HTTPS deployments.

## Re-evaluation Triggers

Re-open only if:

- `PluginSession` gains a trustworthy downstream-scheme primitive (real TLS indicator, not a forwarded header), making auto-derive safe.
- The OIDC plugin migrates `session_cookie_secure` to a different shape (keep the two consistent).

## Reference Cases

- cookie-auth-04 (RFC 6265bis §4.1.2.5 / §5.2.5), closed 2026-05-20
- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a5.md → V-A5-04`
- Config field: `edgion-resources/src/resources/edgion_plugins/plugin_configs/csrf.rs`
- Docs: `docs/en/user-guide/http-route/filters/edgion-plugins/csrf.md`
