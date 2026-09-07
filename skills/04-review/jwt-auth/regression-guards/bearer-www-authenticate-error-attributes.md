---
name: bearer-www-authenticate-error-attributes
description: Read when a finding claims the gateway Bearer WWW-Authenticate header lacks error= / error_description= per RFC 6750 §3.1, or that OAuth clients cannot distinguish invalid-token from no-credentials.
knowledge_type: regression-guards
---

# Gateway Bearer WWW-Authenticate error / error_description attributes

**Status / verdict**: fixed (cookie-auth-02 + rfc-refactor-cookie-auth-01, 2026-05-20)

When reviewing findings of the form "gateway Bearer `WWW-Authenticate` lacks `error=` / `error_description=` per RFC 6750 §3.1 / OAuth clients can't distinguish invalid-token from no-creds": **fixed (cookie-auth-02 + rfc-refactor-cookie-auth-01, 2026-05-20)**. `send_gateway_auth_error_response` (`plugins/http/common/auth_common.rs`) now takes `error_code: Option<&str>`; when `Some` it emits `error="<code>", error_description="<body>"` (body escaped via `escape_quoted_string`, RFC 9110 §5.6.4). Mapping (Approach A — attach a code only on **unambiguous** paths, never on the no-creds path, since emitting `error=` when no credentials were presented is itself an RFC 6750 §3.1 SHOULD NOT): `jwt_auth` verify-failed → `invalid_token`, no-token → `None`; `openid_connect` `send_forbidden` (403, authz/claim failure) → `insufficient_scope`, `send_unauthorized` (401, mixed absent/invalid) → `None`; non-Bearer schemes (`basic`/`key`/`hmac`/`ldap`/`header_cert`) and all `jwe_decrypt` paths → `None`. RFC 6750 §3.1 is **SHOULD** (not MUST). Re-reports asking to also attach `invalid_token` to the OIDC 401 path, or to add a separate `error_description` parameter, are **not accepted** (deliberate Approach A scope cut: OIDC 401 conflates absent/invalid creds; `error_description` reuses the existing body string). Gateway API conformance unaffected (suite does not test `WWW-Authenticate` content). Also **not accepted**: re-reports that `jwt_auth`'s `VerifyDeny::InvalidCredentials` (operator-misconfigured secret/PEM) is mislabeled `invalid_token` and should be a 500 — keeping the generic 401 + `invalid_token` is the intentional, more security-conscious choice (a 500 "secret not configured" would leak server-misconfiguration state to anonymous clients); this is a pre-existing 401-vs-500 classification matter, not part of the `WWW-Authenticate` attribute fix.

## Historical resolution evidence: rfc-refactor-cookie-auth-01

This historical correction covered error attributes, not configurable realm safety.
The shared realm boundary is documented separately in
[Authentication challenge realm quoted strings](../../security/regression-guards/auth-challenge-realm-quoted-strings.md).

`send_gateway_auth_error_response` signature extended with `error_code: Option<&str>` (the discriminator that forces each caller to choose a code or `None`). Closed together with cookie-auth-02 — same change. `error_description` was folded into the existing `body` arg rather than added as a separate parameter.

## Resolution references

- `cookie-auth-02` — 2026-05-20.
- `rfc-refactor-cookie-auth-01` — 2026-05-20.
