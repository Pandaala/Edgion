---
name: jwt-revocation-stateless-tradeoff
description: Use when reviewing local_auth JWT revocation findings; documents why stateless tokens with no server-side blacklist are an accepted tradeoff and how OIDC serves as the revocation path.
---

# Local Admin JWT Has No Server-Side Revocation: Stateless Tradeoff + OIDC as Revocation Path

## Conclusion

Admin JWTs issued in `[local_auth]` mode are **stateless** — `POST /api/v1/auth/logout` only clears the cookie; an issued token remains valid until `exp`, with no server-side blacklist. This is an **already-evaluated architectural tradeoff**; it **should not** require introducing a token blacklist (whether memory or Redis-backed) at the code level.

When a review reports "local admin JWT should have a revocation mechanism / should add a Redis blacklist / should switch to short-lived access token + refresh token", close per the rules below.

## Reasons

1. **Industry consistency**:
   - Argo CD local administrator: pure stateless JWT, no revocation, recommends SSO
   - Kubernetes Dashboard built-in token: same pattern
   - Products that need stateful sessions (Portainer / Grafana / GitLab) take a completely different architecture (DB-backed sessions, DB query per request); not "add a blacklist to JWT"
2. **Architectural dilemma of a gateway-layer blacklist**:
   - **In-memory blacklist (`HashSet<jti>`)**: in multi-instance HA, each instance is independent; logout hits instance A, subsequent requests hit B → B does not know about the revocation → **illusory defense**; cleared on process restart
   - **Redis blacklist**: introduces a hard Redis dependency on the Admin API path (Redis is currently only an optional backend for `rate_limit_redis`, not an Admin API dependency) + 1 RT latency per request + when Redis fails, fail-open (security failure) vs fail-close (availability failure) is a dilemma
3. **Edgion already has a built-in revocation path**: `unified_auth`'s `[auth]` OIDC config delegates session revocation to the IdP (Keycloak / Auth0 / Okta / Azure AD / Google) — the IdP revoking the session/refresh token causes immediate invalidation on the Edgion side. This is the industry-standard path when management-side authentication needs revocation
4. **Documentation makes the tradeoff explicit + provides 3 alternative offboarding ops** (`skills/02-features/02-config/03-auth-bootstrap.md`):
   - Change the password (new logins must use the new password)
   - Rotate `jwt_secret` (**one-click revoke all**, suitable for emergencies)
   - Wait out the window (combined with shortening `jwt_expiry_hours` to 1-4h)
5. **CLI / Dashboard not affected by shortened expiry**: `edgion-cli` `ensure_auth()` re-logs in automatically; the dashboard is a UX impact, not a functional impact

## Decision Rule

When a review/audit report contains the following patterns, close as "architectural tradeoff + already-existing OIDC revocation path":

1. "JWTs issued within 24h before logout remain valid; should add a server-side blacklist"
2. "Should introduce a Redis backend to store revoked jti"
3. "Should switch to a short-lived access token + refresh token model"
4. "Should change the default of `jwt_expiry_hours` to 1h"

Verify three points before closing:

- `local_auth/handlers.rs`'s `logout_handler` still only clears the cookie (no blacklist call)
- `LocalAuthConfig` has no `revocation_backend` / `token_blacklist` field
- `skills/02-features/02-config/03-auth-bootstrap.md` still contains the "JWT revocation semantics" section, making explicit the stateless tradeoff + OIDC path + 3 offboarding ops alternatives

All three satisfied → architectural tradeoff; point to `tasks/security-audit/03-cryptography.md` M11 FIXED (doc-only) decision and close.

## Exceptional Scenarios (need reassessment)

- Industry mainstream (Argo CD / K8s Dashboard) shifts to providing a server-side blacklist for local admin → reassess and follow
- A compliance requirement (PCI-DSS / FIPS / certain industries) mandates that management-side tokens must be instantly revocable and cannot go through OIDC → start a separate project to implement per compliance requirement
- `unified_auth`'s OIDC support is removed or becomes optional/unreliable → reassess (the OIDC revocation path no longer holds at that point)

## Cross-Reference with Other Stateless/Doc-Only Decisions

This decision belongs to the same category as the following rules: "**code keeps the industry stateless pattern + documentation provides alternative paths and config recommendations**":

- `hmac-auth/replay-protection-timestamp-only.md` (M6) — same rejection rationale for HMAC nonce dedup
- `observability/system-error-state-metric.md` (M2) — preserve emergency escape hatch + observability backstop
- `oidc/nonce-pkce-defaults.md` (M8) — doc-only recommendation to opt-in for production

Common pattern: **the code refuses to introduce illusory defense infrastructure of the "distributed consistency / hard dependency" kind**, pointing the path that "truly needs this capability" to existing appropriate mechanisms (OIDC, business-layer idempotency, Alertmanager, etc.).

## Background Notes

- Decision date: 2026-04-22
- Decision source: `tasks/security-audit/03-cryptography.md` M11 (Option E — doc-only)
- Changes this round: `skills/02-features/02-config/03-auth-bootstrap.md` adds a "JWT revocation semantics" section + the doc comment of the `jwt_expiry_hours` field in `src/core/common/local_auth/config.rs` is enhanced. **Zero code behavior change**
- Related references: Argo CD's [Self-Signed JWT for Local Account](https://argo-cd.readthedocs.io/en/stable/operator-manual/user-management/#local-users-and-accounts) is the closest industry analog to Edgion's local admin JWT
