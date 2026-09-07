# D0047 — Align HmacAuth Documentation with the Runtime Wire Format

## Meta
- Status: implemented
- Severity: medium
- Discovered in: B0081, HmacAuth audit
- Category: documentation-correctness
- Closed by: bilingual HmacAuth user-guide alignment; rule [`skills/04-review/hmac-auth/regression-guards/wire-format-is-edgion-contract.md`](../../../../skills/04-review/hmac-auth/regression-guards/wire-format-is-edgion-contract.md)

## Evidence
The English and Chinese HmacAuth user guides advertise an `Authorization: Signature keyId=...` request and say that authentication may read a `Signature` header. The established runtime instead accepts `Authorization` or `Proxy-Authorization`, requires the `hmac` scheme and `username` parameter, and removes `Signature` only as part of `hideCredentials`. The same guides also contain malformed `requestPlugins` list examples, a bare `filters` carrier that is not an EdgionPlugins resource shape, and a sample date that cannot pass the documented timestamp window.

## Why deferred
The runtime wire contract is already covered by source and frozen integration tests. The correction is a coordinated bilingual documentation edit that should update every example and explanation consistently without changing authentication behavior or adding compatibility parsing.

## Integration-test impact
No integration-test issue is required. Existing HmacAuth integration coverage already exercises the implemented request format, signature verification, and body-tamper path; this task aligns documentation with that behavior.
