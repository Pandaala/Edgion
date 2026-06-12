---
name: local-auth-cookie-secure-configurable
description: Use when reviewing local-auth login/logout cookie findings; covers the configurable `cookie_secure` attribute on the `edgion_token` cookie and why a "missing Secure" re-report is stale.
---

# Local-auth `edgion_token` cookie `Secure` is operator-configurable — FIXED

## Conclusion

The local-auth `edgion_token` cookie's `Secure` attribute is driven by `LocalAuthConfig.cookie_secure` (default `true`), not omitted. Re-reports of the form "`handlers.rs` login/logout set the cookie with `HttpOnly; SameSite=Strict; Path=/` but no `Secure`, so the bearer-equivalent JWT leaks over plaintext `http://`" are **stale** — closed as common-center-02 (2026-05-24).

Fix suggestions of the form "always emit `Secure` unconditionally" or "auto-derive `Secure` from `x-forwarded-proto`" are **not accepted**.

## Core Rationale

**1. Configurable, default-secure**

Both `login_handler` and `logout_handler` (`edgion-common/src/local_auth/handlers.rs`) build the cookie with a conditional `Secure` segment driven by `LocalAuthState.cookie_secure` (sourced from `LocalAuthConfig.cookie_secure`, default `true`). The secure-by-default behavior is preserved; operators terminating TLS upstream (Ingress/sidecar) or running mesh-internal plain HTTP set `local_auth.cookie_secure = false`.

**2. Logout reads the same flag**

`logout_handler` is no longer stateless — it takes `State<Arc<UnifiedAuthState>>` and reads `cookie_secure` so the clearing cookie carries the **same** `Secure` attribute as the login cookie. This matters: a `Secure` clearing cookie sent over plain HTTP would be refused by the browser, leaving the cookie un-cleared. When the local provider is not yet populated (`state.local` is `None`), it falls back to secure-by-default (`true`).

**3. Consistent with CSRF / OIDC**

The plain-`bool`, default-`true`, operator-set shape mirrors the existing CSRF `cookie_secure` and OIDC `session_cookie_secure` precedents — see [../csrf/cookie-secure-configurable.md](../csrf/cookie-secure-configurable.md). A flag does not trust the client-suppliable `x-forwarded-proto` header.

## Fix Suggestions Not Accepted

- **Unconditional `Secure`** — breaks explicit non-TLS dev/internal deployments served over plain HTTP (the browser refuses to store a `Secure` cookie over `http://`, except for `localhost` secure-context special-casing).
- **Auto-derive from `x-forwarded-proto`** — trusts a client-controllable header and diverges from the CSRF/OIDC config precedent.

## Re-evaluation Triggers

Re-open only if:

- The Admin API gains a trustworthy downstream-scheme primitive (real TLS indicator, not a forwarded header), making auto-derive safe.
- The CSRF / OIDC plugins migrate their `cookie_secure` / `session_cookie_secure` flags to a different shape (keep all three consistent).

## Reference Cases

- common-center-02 (review-2), closed 2026-05-24
- Config field: `edgion-common/src/local_auth/config.rs` (`cookie_secure`, `default_cookie_secure()`)
- Handlers: `edgion-common/src/local_auth/handlers.rs` (`login_handler`, `logout_handler`)
- Docs: `docs/en/ops-guide/configuration/controller-config.md` → `[local_auth]`
- Related: [jwt-revocation-stateless-tradeoff.md](jwt-revocation-stateless-tradeoff.md) — leaked-cookie revocation goes through `jwt_secret` rotation; the `Secure` flag is the prevention-side complement.
