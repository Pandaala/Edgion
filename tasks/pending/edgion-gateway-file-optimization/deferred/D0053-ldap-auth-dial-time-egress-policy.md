# D0053 — Enforce LDAP Dial-time Egress Policy

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0088, LDAP Auth audit
- Category: egress-security
- Canonical task: [`tasks/todo/ldap-auth-dial-time-egress-policy.md`](../../../todo/ldap-auth-dial-time-egress-policy.md)

## Evidence
`ldapHost` is operator-controlled, but each authenticated request gives an `ldap://` or `ldaps://` URL to ldap3, which performs system DNS resolution and connection establishment without `EgressResolver` or `classify_ip`. The shared dial-time egress policy therefore cannot classify every resolved candidate or block private, loopback, link-local, or otherwise denied destinations.

## Why deferred
The correct fix needs a shared non-HTTP resolution and dial seam that classifies every candidate while preserving the original hostname for LDAPS and StartTLS SNI and certificate verification. It must also define multi-address fallback, connect timeout, error classification, and all-blocked fail-close behavior. A local URL rewrite or preflight-only lookup would introduce DNS rebinding and TLS identity regressions.

## Integration-test impact
D0054 tracks the required new cross-process coverage after the transport design is implemented. Existing frozen LDAP integration files remain unchanged.
