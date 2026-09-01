# D0050 — Parse JWE Protected Headers as Strict JSON Objects

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0084, JweDecrypt audit
- Category: parsing-correctness
- Canonical task: [`tasks/todo/harden-jwe-protected-header-json-parsing.md`](../../../todo/harden-jwe-protected-header-json-parsing.md)

## Evidence
The current `serde_json::Value` parse accepts duplicate protected-header member names with last-value-wins semantics and classifies a valid non-object JSON value as missing `alg`/`enc`. RFC 7516 processing needs one unambiguous protected-header object while still authenticating the original encoded segment as AAD.

## Why deferred
A correct change needs a strict duplicate-detecting object parser, exact failure precedence, and regression coverage for known and unknown duplicate members. B0084 intentionally hardened only unsupported `zip`/`crit` presence without broadening its parser boundary.

## Integration-test impact
No integration-test issue is required. Focused parser and request-filter unit tests can cover duplicate members, non-object JSON, raw AAD, and unchanged status/log behavior.
