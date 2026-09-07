---
name: jwt-decoding-key-prebuilt
description: Use when reviewing JwtAuth credential-store or verification changes that could rebuild DecodingKey values on the request hot path.
status: fixed
finding_id: gw-plugin-auth-a-02
closed: 2026-08-02
knowledge_type: regression-guards
---

# JwtAuth `DecodingKey` construction — fixed

## Conclusion

`JwtIssuerGroupStore` builds every credential's `DecodingKey` when the
immutable plugin store is constructed. The request path selects a cached key
by `iss` and `kid`; it must not decode base64 secrets or parse RSA/EC PEM again.

Key-construction errors are cached alongside successful keys. A request that
selects a malformed credential therefore keeps the existing fail-closed
behavior: `VerifyDeny::InvalidCredentials`, a non-sensitive PluginLog tag, and
the plugin's generic 401 response. Store construction does not fail Gateway
startup or drop the entire plugin because one resolved credential is malformed.

## Security invariants

- Route by exact `iss` before selecting a cached key.
- Select `kid` only from the matched issuer group's cache.
- Compare the JWT header algorithm with the group's configured algorithm before
  using the cached key.
- Never include key parse errors, issuer values, or key identifiers in a denial
  log or client response.

## Regression coverage

The credential-store unit test verifies that malformed RSA PEM is parsed at
store construction and retained as a cached error. The existing JwtAuth tests
cover issuer routing, cross-group `kid` isolation, algorithm-confusion denial,
base64 HMAC secrets, and generic authentication failures.

## Re-evaluation triggers

Re-open if JwtAuth gains runtime key rotation independent of plugin-store
replacement, or if verification starts constructing a `DecodingKey` from raw
credential material after request dispatch.

## Resolution references

- `gw-plugin-auth-a-02` — 2026-08-02.
