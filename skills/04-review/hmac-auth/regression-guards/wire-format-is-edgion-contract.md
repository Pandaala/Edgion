---
name: wire-format-is-edgion-contract
description: Use when reviewing HmacAuth request-format findings; the user guides document Edgion's runtime contract, not Kong, APISIX, draft-cavage, RFC 9421, or Envoy encoding.
status: fixed
finding_id: D0047
closed: 2026-09-05
knowledge_type: regression-guards
---

# HMAC Auth Wire Format Is Edgion's Contract — FIXED

## Conclusion

HmacAuth accepts `Authorization` or, if that header is absent, `Proxy-Authorization`, with scheme `hmac` and a `username` parameter. User guides must describe that contract. Feature-level alignment with other HMAC gateways (timestamp window, signed components, optional body digest) is enough; wire encoding does not have to match another product.

Fix suggestions of the form "document or parse `Authorization: Signature keyId=...`", "accept a standalone `Signature` header for authentication", "implement RFC 9421 `Signature-Input`", or "make the header encoding Kong/APISIX/Envoy-identical" are **not accepted**.

## Core Rationale

**1. Runtime contract is already frozen**

`parse_authorization` requires scheme `hmac` (ASCII case-insensitive) and quoted `username`, `algorithm`, `headers`, and `signature` parameters. Verification reads `authorization`, then `proxy-authorization`. A standalone `Signature` header is never an authentication source. After a successful authentication, `hideCredentials` removes `authorization`, `proxy-authorization`, and `signature` before proxying so leftover draft-cavage headers are not forwarded. The anonymous missing-auth path does not run this cleanup. Header names in the `headers` parameter are lowercased before the signing string is built.

**2. Industry products share the feature, not one encoding**

Kong hmac-auth happens to use `hmac` + `username`. APISIX and draft-cavage use `Signature keyId=...`. RFC 9421 uses `Signature-Input` / `Signature` structured fields. Envoy has no inbound HMAC request-auth filter. Copying any one of those encodings would either break existing Edgion clients or imply a compatibility surface the runtime does not provide.

**3. Documentation is the correction surface**

`docs/en/user-guide/http-route/filters/edgion-plugins/hmac-auth.md` and the Chinese counterpart describe Edgion's request format, signing-string construction, and the narrow `hideCredentials` strip of `Signature`. They do not claim HTTP Signature RFC compatibility.

## Fix Suggestions Not Accepted

- Add `Signature` / `keyId` parsing or aliases — expands the auth surface and contradicts the frozen parser
- Document draft-cavage or RFC 9421 as the Edgion request format — clients following that text receive 401
- Change header precedence, algorithm set, or signing-string rules to match Kong/APISIX — those are implementation details, not product requirements
- Edit frozen HmacAuth integration tests as part of a documentation alignment — runtime behavior is unchanged

## Re-evaluation Triggers

Re-open this decision only if:

- The runtime parser is intentionally changed to accept another wire encoding
- Edgion adds a separately designed RFC 9421 or draft-cavage authenticator

## Reference Cases

- D0047 / `tasks/todo/align-hmac-auth-wire-format-documentation.md` (closed by this documentation alignment)
- `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs` (`parse_authorization`, `verify_headers_with`, `hide_credentials`)
- `edgion-tests/src/client/suites/edgion_plugins/hmac_test_utils.rs`

## Resolution references

- `D0047` — 2026-09-05.
