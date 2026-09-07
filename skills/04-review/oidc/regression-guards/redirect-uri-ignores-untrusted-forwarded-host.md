---
name: redirect-uri-ignores-untrusted-forwarded-host
description: Use when reviewing OIDC redirect URI host derivation; path-form redirect targets use the routed Host authority and never trust X-Forwarded-Host without a proxy trust boundary.
status: fixed
finding_id: gw-plugin-oidc-02
closed: 2026-08-02
knowledge_type: regression-guards
---

# OIDC Path Redirect URIs Ignore Untrusted `X-Forwarded-Host` — FIXED

## Conclusion

Path-form `redirectUri` and `postLogoutRedirectUri` values are expanded with the request `Host`. The OIDC plugin ignores `X-Forwarded-Host` because Edgion has no trust configuration for that header. Operators whose public origin differs from the routed authority must configure an absolute URL.

Fix suggestions that restore unconditional `X-Forwarded-Host` precedence are **not accepted**.

The scheme half of the same threat model is decided in
[redirect-uri-scheme-from-connection-not-forwarded-proto.md](redirect-uri-scheme-from-connection-not-forwarded-proto.md):
`request_scheme` derives `https`/`http` from the listener's real TLS state and ignores
`X-Forwarded-Proto`. The two helpers must not diverge.

## Core Rationale

**1. Forwarded host has no trust boundary**

Clients can submit `X-Forwarded-Host` directly, and the gateway does not authenticate its source or strip it before plugin execution. Using it for an IdP-facing redirect parameter lets an untrusted header replace the authority already used for gateway routing.

**2. Both redirect consumers share one host decision**

`resolve_redirect_uri` uses the result for authorization and token exchange, while `resolve_post_logout_redirect_uri_for_idp` uses it for RP-Initiated Logout. Keeping the rule in `request_host` prevents the two paths from drifting.

**3. Absolute URLs cover deployments with a different public origin**

Both configuration fields already accept absolute HTTP or HTTPS URLs and bypass request-derived host construction. This avoids adding a new proxy-trust configuration surface solely for OIDC redirect generation.

## Fix Suggestions Not Accepted

- Trust `X-Forwarded-Host` by default — preserves the client-controlled authority override.
- Fall back to `X-Forwarded-Host` when `Host` is absent — requests without a valid authority are rejected by the gateway and must not gain a separate OIDC fallback.
- Add an OIDC-only trusted-proxy or opt-in flag — duplicates proxy trust policy and expands the public configuration surface when absolute URLs already provide an explicit origin.

## Re-evaluation Triggers

Re-open this decision only if:

- The gateway introduces a canonical trusted forwarded-host mechanism shared by every consumer.
- OIDC redirect generation receives a trusted, route-derived external authority that is distinct from the request `Host`.
- Absolute redirect URLs stop being supported.

## Reference Cases

- `gw-plugin-oidc-02` (fixed 2026-08-02).
- `edgion-gateway/src/plugins/http/openid_connect/config_validation.rs`.
- `docs/en/user-guide/edgion-plugins/openid-connect.md`.

## Resolution references

- `gw-plugin-oidc-02` — 2026-08-02.
