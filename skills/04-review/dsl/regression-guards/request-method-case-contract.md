---
name: request-method-case-contract
description: Use when reviewing DSL request method casing, validation, or documentation promises.
knowledge_type: regression-guards
---

# DSL request methods preserve case

## Conclusion and scope

`req.set_method` preserves the supplied HTTP method token, and `req.method`
reads the current method with that case, including prior rewrites. `POST`,
`post`, and `Post` are distinct tokens. Standard methods use their uppercase
spelling; arbitrary extension methods are not normalized.

The correction is to the English and Chinese API reference, which previously
promised case-insensitive writes with uppercase conversion and always-uppercase
reads. The user approved preserving runtime behavior and correcting that promise.
No production API, parsing, or forwarding behavior changed.

## Rationale and evidence

[RFC 9110 section 9.1](https://www.rfc-editor.org/rfc/rfc9110.html#section-9.1)
defines case-sensitive method tokens and the uppercase convention for standard
methods. This protocol basis supports the resolved contract independently of
current implementation behavior.

- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`: `ReqSetMethod`
  passes the string unchanged to `PluginSession::set_upstream_method`;
  `ReqMethod` reads `get_method()`. Successful mutation returns `Value::Nil`;
  host failure becomes `RuntimeError::ApiError` with function `req.set_method`.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`: the setter parses
  `http::Method` before mutating the request header. Invalid input therefore
  leaves the existing method intact. The getter returns `method.as_str()`.
- Locked `http` 1.5.0, `src/method.rs`: `FromStr` delegates to `from_bytes`.
  Exact uppercase standard names use dedicated variants; other valid tokens
  use inline (up to 15 bytes) or allocated extensions. `write_checked` validates
  ASCII token bytes without changing case. Empty strings, whitespace, controls,
  `/`, and non-ASCII bytes are rejected.
- Locked Edgion Pingora commit `edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`
  (re-verified by source inspection on 2026-11-06; originally recorded at `518f7274`):
  `pingora-http/src/lib.rs::RequestHeader::set_method` assigns `base.method`.
  `pingora-core/src/protocols/http/v1/client.rs::http_req_header_to_wire`
  writes `method.as_str().as_bytes()` into the request line without conversion.

The [builtin error contract](../../../03-coding/dsl/builtin-error-semantics.md)
continues to govern `Nil`, `ApiError`, and configured error-policy handling.

## Alternatives and limits

Automatically uppercasing arbitrary methods could change extension-method
semantics. Adding convenience aliases for standard methods would also change
the script API and requires a separate explicit decision; the documentation
correction does not authorize it. `http.call` is a separate bounded outbound
exchange and is outside this request-mutation contract.

The new tests exercise the actual DSL VM, adapter, locked parser, Pingora setter,
and HTTP/1 header serializer using an in-memory duplex session. They do not
establish backend acceptance, end-to-end routing, or HTTP/2 wire behavior.
Existing integration tests remain unchanged.

## Verification and re-evaluation

Focused tests in `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/request_mutation.rs`:

- `test_request_method_case_preserved_through_real_adapter`: uppercase,
  lowercase, mixed-case, short and long extension tokens; successful `Nil`,
  DSL readback, distinction from standard `POST`, and HTTP/1 request-line bytes.
- `test_request_method_invalid_input_preserves_method_and_raises_api_error`:
  empty, whitespace, separator, CRLF and non-ASCII inputs; exact `ApiError`
  function and preservation of the previous method.

Verification on 2026-09-13:

- `cargo test -p edgion-gateway --lib plugins::http::dsl::tests`: 375 passed.
- `cargo test -p edgion-gateway --lib plugins::runtime::session_adapter -- --quiet`:
  122 passed.
- `cargo fmt --all` and `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with repository warnings;
  no warning referenced the changed request-mutation test file.
- Agent-document validation, SSA force guard, metrics inventory guard, and
  Pingora source policy guard: passed.
- Independent read-only review: LGTM.

Re-evaluate if the method parser dependency, adapter setter/getter, Pingora
serialization, or script-facing normalization requirements change.

## Resolution references

- `clarify-dsl-request-method-case-contract`: originally an unverified
  dependency-normalization evidence gap; resolved as a documentation defect
  with focused regression coverage, preserving production semantics.
