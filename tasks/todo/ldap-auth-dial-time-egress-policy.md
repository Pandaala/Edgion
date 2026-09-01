# Enforce LDAP Dial-time Egress Policy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | security-design |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0088 |

## AI guide summary

Route LDAP Auth DNS resolution and connection establishment through the canonical egress policy at dial time without changing LDAP hostname identity, TLS verification, or failure policy.

## Requirements

- Design or reuse a shared non-HTTP resolver/dial seam; do not add an LDAP-only DNS cache or duplicate resolver library.
- Resolve and classify every candidate address at connection time through `EgressResolver` or the canonical `classify_ip` policy.
- Preserve the original configured hostname for LDAPS and StartTLS SNI and certificate verification when connecting to an approved resolved address.
- Define deterministic multi-address selection, fallback, connect-timeout budgeting, DNS failure, partially blocked, and all-blocked behavior.
- Fail closed when no approved candidate remains and expose only static redacted PluginLog/access-log diagnostics and existing bounded metrics.
- Preserve per-request LDAP bind behavior, custom CA, SAN pinning, mTLS, authentication 401 behavior, and infrastructure 503 behavior.
- Disable proxy-environment influence and avoid preflight-only checks vulnerable to resolution changes.
- Coordinate the implementation with the standalone integration task; do not modify existing integration tests.

## Acceptance criteria

- No LDAP connection can reach a destination denied by the effective egress policy, including through DNS rebinding or multi-address fallback.
- Allowed LDAPS and StartTLS connections validate the original hostname and retain current CA, SAN, and mTLS behavior.
- Partial and all-blocked resolution outcomes are deterministic, bounded by the configured timeout, and fail closed without sensitive detail.
- Unit tests cover address classification and fallback; D0054 cross-process coverage passes.
- Workspace format/check/Clippy and repository guards pass.
