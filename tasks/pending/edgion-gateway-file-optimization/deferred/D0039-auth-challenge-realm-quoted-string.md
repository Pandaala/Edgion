# D0039 — Harden Auth Challenge Realm Quoted Strings

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0067, BasicAuth audit
- Category: correctness-security
- Canonical task: [`tasks/todo/harden-auth-challenge-realm-quoted-strings.md`](../../../todo/harden-auth-challenge-realm-quoted-strings.md)

## Evidence
The canonical auth error helper escapes `error_description` but interpolates configurable realm text directly into an HTTP quoted-string. Quote, backslash, or control values can produce a malformed or ambiguous challenge across multiple auth consumers.

## Why deferred
The fix belongs in the shared challenge helper plus uniform resource/admission validation. A BasicAuth-only escape would create divergent header contracts.

## Integration-test impact
None required. Shared helper and schema/validation unit tests can prove escaping or rejection consistently for every caller.
