# Reject Unsafe DynamicExternalUpstream SNI and Host Overrides

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | security-correctness |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0073 |

## AI guide summary

Reject explicit SNI and Host override values that cannot safely reach the DynamicExternalUpstream runtime, including the empty-SNI path with `tls: true` that disables upstream certificate verification.

## Requirements

- Reject explicit empty `sni`; absent SNI must continue to default to the configured domain.
- Validate nonempty explicit SNI through one canonical predicate shared with domain validation, while retaining deliberately supported internal names and IP forms unless an approved contract says otherwise.
- Require explicit `overrideHost` to be nonempty, a valid HTTP header value, and a valid Host authority.
- Use fixed validation messages that do not echo configured values.
- Preserve shared Controller/Gateway validation parity and reject invalid configurations before runtime publication.
- Update the EN/ZH DynamicExternalUpstream configuration references atomically.
- Do not change the Gateway runtime, Pingora TLS behavior, schema field names, or existing integration tests.

## Acceptance criteria

- `tls: true` can never publish an `ExternalJumpPreset` with an empty SNI.
- Invalid SNI and Host override values produce stable validation errors before publication.
- Absent SNI, valid internal names, supported IP forms, and valid Host authorities retain current behavior.
- Resource tests, plugin compilation validation, bilingual docs, workspace checks, and repository guards pass.
