# Add OpenID Connect Egress Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Add new end-to-end scenarios proving the D0056 OIDC egress implementation blocks denied DNS and proxy routes while preserving allowed discovery, JWKS, token, and SAN-sensitive TLS flows. D0056 is implemented; the current Gateway campaign must not modify existing integration tests.

## Requirements

- Add coverage only after the OIDC egress policy, resolver injection, and failure contract are implemented.
- Add standalone scenarios and only the minimal new registration or fixture wiring; do not weaken or rewrite existing OpenID Connect assertions.
- Exercise both the default shared-client branch and the SAN-pinning raw-client branch with proxy environment variables present, proving neither branch uses them.
- Exercise allowed multi-address DNS with denied candidates, all-denied resolution, and a discovery document whose JWKS or token endpoint resolves to a denied destination.
- Exercise an allowed hostname through discovery, JWKS, and token exchange while preserving original-hostname TLS and SAN verification.
- Assert deterministic fail-close status and static public diagnostics without endpoint, address, proxy, token, cookie, claim, or client-secret leakage.
- Use local fixtures and readiness/state polling without fixed sleeps or public network dependencies.

## Acceptance criteria

- Denied candidates and proxy routes are never contacted in either client branch.
- All-denied configured or discovery-derived endpoints fail closed without sensitive detail.
- Allowed discovery, JWKS, and token flows complete in both ordinary and SAN-pinning modes with hostname verification intact.
- New scenarios pass repeatedly without rewriting pre-existing scenario assertions or fixtures; any existing-file edit is limited to approved minimal registration wiring.

## Dependency

Ready: D0056 dial-time egress is implemented. Add new standalone scenarios; do not modify existing OpenID Connect integration tests.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Create future standalone scenarios instead of editing existing OIDC integration tests in B0090. | Real DNS, proxy, TLS, and discovered-endpoint routing cannot be proven by the borrowed-JWK unit change. |
