# D0037 — Unify Auth Identity-header Sanitization

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0067, BasicAuth audit
- Category: correctness-security
- Canonical task: [`tasks/todo/unify-auth-identity-header-sanitization/`](../../../todo/unify-auth-identity-header-sanitization/unify-auth-identity-header-sanitization.md)

## Evidence
Multiple auth plugins can leave client-supplied gateway identity headers such as `X-Consumer-Username` or `X-Anonymous-Consumer` on authenticated success or across authenticated/anonymous transitions. LDAP clears its configurable anonymous marker, but the broader ownership contract is not canonical across plugins or plugin ordering.

## Why deferred
Correctness spans BasicAuth, KeyAuth, JwtAuth, HmacAuth, HeaderCertAuth, LDAP, session mutation, invalid resolved header values, and multiple-auth-plugin order. A BasicAuth-local removal would leave divergent spoofing behavior.

## Integration-test impact
D0038 records the required new upstream-observation scenario. Existing integration tests remain frozen during this campaign.
