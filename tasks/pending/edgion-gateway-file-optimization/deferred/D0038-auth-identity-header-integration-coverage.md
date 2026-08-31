# D0038 — Add Auth Identity-header Spoofing Integration Coverage

## Meta
- Status: blocked
- Severity: high
- Discovered in: B0067, BasicAuth audit
- Category: testing
- Canonical task: [`tasks/todo/add-auth-identity-header-spoofing-integration-coverage.md`](../../../todo/add-auth-identity-header-spoofing-integration-coverage.md)
- Dependency: D0037 / canonical auth identity-header sanitization design and implementation

## Evidence
Mocks can prove local header mutation but cannot prove which identity headers reach the real upstream after route/plugin composition, authenticated/anonymous transitions, and multiple auth plugins.

## Why deferred
The scenario must consume the canonical ownership and ordering contract chosen by D0037. Adding it first would encode guessed semantics, while modifying current integration files would violate the frozen-test contract.

## Candidate coverage
Add new standalone scenarios that send forged identity headers through authenticated success, anonymous allowance, failed auth, and multiple-plugin ordering, then assert the upstream receives only canonical gateway-owned values or no identity header as specified. Use existing readiness/state polling and no fixed sleeps.
