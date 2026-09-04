# D0053 — Enforce LDAP Dial-time Egress Policy

## Meta
- Status: resolved
- Severity: high
- Discovered in: B0088, LDAP Auth audit
- Category: egress-security
- Closed: 2026-09-04
- Canonical record: [`skills/04-review/ldap-auth/dial-time-egress-policy.md`](../../../../skills/04-review/ldap-auth/dial-time-egress-policy.md)

## Evidence
Before the fix, `ldapHost` was passed to ldap3 as an `ldap://` or `ldaps://` URL. ldap3 performed system DNS and `TcpStream::connect(host:port)` without `EgressResolver` or `classify_ip`, so dial-time policy could not classify every candidate or block denied destinations.

## Resolution
`LdapAuth` now holds a Gateway `EgressResolver`, classifies every candidate at connect time, dials only admitted addresses, and keeps the original hostname URL for LDAPS/StartTLS. URL rewrite and preflight-only DNS were rejected.

## Integration-test impact
D0054 remains the standalone cross-process coverage task. Existing frozen LDAP integration files remain unchanged.
