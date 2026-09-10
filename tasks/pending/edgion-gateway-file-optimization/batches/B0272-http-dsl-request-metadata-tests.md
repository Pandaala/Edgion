# B0272 — HTTP DSL Request Metadata Tests

## Scope

- Pipeline state: analysis-in-progress, queued behind B0271; no implementation authorized.
- Active anchor: `edgion-gateway`; baseline `209cd8c1`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/req_metadata_tests.rs`.
- Read scope may follow metadata builtin dispatch, session abstractions and related tests. Existing integration tests and user-owned handoff/WAF paths remain protected.

## Analysis

- Dedicated read-only value analysis pending; refresh against HEAD before promotion.
