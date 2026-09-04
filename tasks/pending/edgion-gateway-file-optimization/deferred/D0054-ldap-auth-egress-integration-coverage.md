# D0054 — Add LDAP Egress and TLS Integration Coverage

## Meta
- Status: pending
- Severity: high
- Discovered in: B0088, LDAP Auth audit
- Category: testing
- Canonical task: [`tasks/todo/add-ldap-auth-egress-integration-coverage.md`](../../../todo/add-ldap-auth-egress-integration-coverage.md)
- Dependency: D0053 is implemented; this task owns the new cross-process coverage

## Evidence
Unit tests can validate resolution and address-classification components, but cannot prove that a fixed-address LDAP connection preserves the original hostname through real LDAPS and StartTLS SNI/certificate verification or that all-blocked DNS results fail closed through the complete Gateway path.

## Why deferred
D0053 is implemented. Remaining work is new standalone scenarios; modifying current LDAP integration files would still violate the campaign's frozen-test contract.

## Candidate coverage
Add a new standalone dynamic scenario that exercises allowed multi-address resolution, blocked and all-blocked candidates, LDAPS and StartTLS with hostname-sensitive certificates, and deterministic fail-close diagnostics. Use the existing LDAP test process and readiness/state polling without fixed sleeps.
