# D0051 — Align JWE Resolved-Credential Failure Policy

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0084, JweDecrypt call-chain review
- Category: authentication-failure-policy
- Canonical task: [`tasks/todo/align-jwe-resolved-credential-failure-policy.md`](../../../todo/align-jwe-resolved-credential-failure-policy.md)

## Evidence
Invalid outer Base64, inner Base64, or decoded key length in Controller-resolved operator Secret material fails shared plugin validation before `JweDecrypt` construction. `compile_plugins` therefore installs `RejectInvalidConfig`, producing the current generic invalid-plugin HTTP 500 instead of a generic authentication denial. This conflicts with the auth credential-failure policy and the existing JwtAuth malformed-key precedent.

## Why deferred
The correction crosses resource validation, Controller status signaling, Gateway generation preparation, last-known-good publication behavior, and request-time authentication semantics. Removing validation locally would lose operator feedback; changing only the response would leave malformed material outside the prepared runtime model.

## Integration-test impact
No integration-test issue is opened now. The design should first define status and publication semantics, then decide whether focused cross-crate tests fully cover Secret rotation and malformed-material recovery without modifying existing integration tests.
