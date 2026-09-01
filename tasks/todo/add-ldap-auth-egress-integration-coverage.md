# Add LDAP Egress and TLS Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | blocked |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Add new end-to-end LDAP Auth scenarios proving the D0053 dial-time egress implementation blocks denied addresses while preserving real LDAPS and StartTLS hostname verification. This task is blocked on [`ldap-auth-dial-time-egress-policy`](ldap-auth-dial-time-egress-policy.md); the current Gateway campaign must not modify existing integration tests.

## Requirements

- Add coverage only after the canonical LDAP egress transport seam and failure contract are implemented.
- Add new standalone scenario files; do not weaken or rewrite existing LDAP integration assertions.
- Exercise an allowed hostname resolving to multiple candidates, including a denied candidate that must never be dialed.
- Exercise all-blocked resolution and assert the fixed fail-close status, static diagnostic classification, and absence of internal addresses in public responses.
- Exercise both LDAPS and StartTLS through an approved fixed address while validating the original configured hostname with hostname-sensitive certificates.
- Include the current custom CA, SAN policy, and mTLS identity where the D0053 design makes them applicable.
- Use readiness/state polling and explicit resolver fixtures; do not use fixed sleeps or public network dependencies.

## Acceptance criteria

- Denied candidates are never contacted and an allowed fallback works deterministically.
- All-blocked resolution fails closed without leaking DNS candidates or connection detail.
- LDAPS and StartTLS preserve original-hostname SNI and certificate verification through the egress-aware dial path.
- New scenarios pass repeatedly without modifying existing integration tests.

## Dependency

Blocked on [`Enforce LDAP Dial-time Egress Policy`](ldap-auth-dial-time-egress-policy.md).

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Create a separate future integration issue instead of changing tests in B0088. | Unit tests cannot validate address filtering plus original-hostname LDAPS and StartTLS behavior through the real Gateway and LDAP process. |
