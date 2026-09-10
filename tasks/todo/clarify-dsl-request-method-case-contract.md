# Clarify the DSL request-method casing contract

## Finding

The English DSL API reference promises case-insensitive method names converted
to uppercase (`docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md:648`).
The verified Gateway call chain contains no explicit normalization:

- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs:626` passes the string unchanged to `set_upstream_method`.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs:1216` parses it as `http::Method` and installs the parsed method.
- `edgion-gateway/src/plugins/http/dsl/tests/request_mutation.rs` tests uppercase POST through a mock, not actual adapter casing behavior.

Dependency-level normalization has not been verified. This is a documentation-to-implementation
evidence gap, not yet proof of incorrect forwarded bytes. Script authors may rely
on the documented conversion for lowercase or mixed-case names.

## Scope

Trace the locked `http::Method` parser and supported Pingora setter; establish the
intended contract for standard and extension method tokens against project authority.
Current behavior alone does not establish the requirement. Correct documentation if
the uppercase promise is stale; do not uppercase arbitrary extension methods without
justifying their semantics. Preserve successful Nil and host-failure ApiError contracts.

## Acceptance and verification

- Record dependency-source evidence for uppercase, lowercase, mixed-case, valid extension and invalid inputs.
- Align English and corresponding Chinese user documentation with the resolved contract.
- Add focused non-network unit coverage for changed behavior or an uncovered contract boundary; do not change production solely for mocks.
- Keep existing integration tests unchanged; track necessary new end-to-end coverage separately.
- Run relevant DSL/adapter tests and applicable checks; documentation-only work includes agent-document validation.
