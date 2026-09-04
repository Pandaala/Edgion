# D0057 — Add OpenID Connect Egress Integration Coverage

## Meta
- Status: pending
- Severity: high
- Discovered in: B0090, OpenID Connect audit
- Category: testing
- Canonical task: [`tasks/todo/add-openid-connect-egress-integration-coverage.md`](../../../todo/add-openid-connect-egress-integration-coverage.md)
- Dependency: D0056 is implemented; this task owns the new cross-process coverage

## Evidence
Unit tests can validate resolver and client construction, but cannot prove that both the default and SAN-pinning clients ignore proxy environment variables, classify the actual dial address after DNS resolution, block private or metadata destinations, and still complete a real discovery/JWKS/token flow when policy permits the identity provider.

## Why deferred
D0056 is implemented. Remaining work is new standalone scenarios; modifying current OpenID Connect integration files would still violate the campaign's frozen-test contract.

## Candidate coverage
Add new standalone scenarios for the default and SAN-pinning client branches. Cover proxy-environment isolation, allowed multi-address resolution, denied and all-denied candidates, discovery-derived endpoints, hostname-sensitive TLS/SAN verification, deterministic fail-close responses, and readiness/state polling without fixed sleeps.
