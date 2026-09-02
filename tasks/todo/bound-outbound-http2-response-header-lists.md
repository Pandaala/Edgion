# Bound Outbound HTTP/2 Response Header Lists

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-02 |
| Status | pending |
| Type | security |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0107 analysis |

## AI guide summary

Define and enforce one decoded HTTP/2 response-header-list bound in the canonical outbound HTTP client for all reqwest consumers.

## Requirements

- Inventory every `build_outbound_client` consumer and the current Hyper/H2 decoded-header limits.
- Select one bounded default with explicit interoperability and memory-amplification evidence.
- Enforce the bound before consumer-level response materialization and map failures without leaking dependency detail.
- Keep proxy disabling, redirects, egress checks, TLS resolution, body streaming caps, and consumer deadlines unchanged.
- Do not add a plugin-local parallel cap or modify existing integration-test files.

## Acceptance criteria

- Every canonical reqwest client applies the same documented decoded HTTP/2 response-header-list bound.
- Oversized response headers fail with bounded memory and generic external errors while preserving operational diagnostics.
- Shared-client unit tests cover the exact boundary; any required new integration scenario is tracked separately.
- Workspace format, check, Clippy, documentation, observability, source-policy, and frozen-path guards pass.
