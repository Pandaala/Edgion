# Enforce OpenID Connect Dial-time Egress Policy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | security-design |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0090 |

## AI guide summary

Apply one explicit dial-time egress policy to every OpenID Connect outbound endpoint and both TLS client-construction branches without breaking legitimate internal identity providers or hostname verification.

## Requirements

- Define the operator-visible policy and default for configured and discovery-derived OIDC destinations; do not silently block existing private identity providers without that product decision.
- Route the default shared-client path and the SAN-pinning raw-client path through the same Gateway `EgressResolver` and effective egress policy.
- Disable proxy-environment pickup in both branches and continue denying redirects.
- Classify every resolved candidate at dial time, including discovery, JWKS, authorization support calls, token exchange and refresh, introspection, userinfo, and revocation endpoints.
- Preserve original hostnames for TLS SNI, certificate verification, custom CA, and SAN policy while connecting only to approved resolved addresses.
- Define deterministic multi-address fallback, timeout budgeting, DNS failure, partial denial, all-denied behavior, pooling, and cache interaction.
- Fail closed with static public responses and redacted PluginLog/access-log diagnostics; never expose endpoints, resolved addresses, proxy details, tokens, cookies, claims, or client credentials.
- Do not modify existing integration tests; coordinate the implementation with the standalone blocked integration task.

## Acceptance criteria

- No OIDC client branch can reach a destination denied by the effective policy through DNS rebinding, discovered endpoints, multi-address fallback, or proxy environment variables.
- Allowed internal and external identity providers retain existing discovery, JWKS, token, introspection, userinfo, revocation, TLS, SAN, timeout, redirect, and pooling behavior.
- Default and SAN-pinning client factories share one documented resolver/policy contract rather than separate partial defenses.
- Unit tests cover construction and address classification; D0057 end-to-end coverage passes.
- Workspace format/check/Clippy and repository guards pass.
