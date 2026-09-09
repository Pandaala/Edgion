# Correct DSL Response-stage gRPC Error Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | docs |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Correct the bilingual DSL guides to describe the supported response-stage `grpc_error()` behavior instead of the obsolete request-only degradation model.

## Requirements

- Audit the five maintained DSL user-guide files under both `docs/en/` and `docs/zh-CN/`: `README.md`, `api-reference.md`, `examples.md`, `language-syntax.md`, and `sandbox-security.md`.
- State that request-stage `grpc_error()` emits the canonical direct trailers-only gRPC response.
- State that response-stage `grpc_error()` aborts the upstream response and carries the exact gRPC code and optional message to the canonical terminal writer.
- Preserve the independent status model: the wire HTTP status is 200 while the gRPC application status is recorded separately in `grpc_status`.
- Remove obsolete claims that response-stage use degrades to `terminate()`, is request-only, or merely emits a warning log.
- Keep English and Chinese documentation semantically aligned and verify every example against current runtime and terminal-writer behavior.
- Do not change production code, schemas, logging, metrics, or integration tests.

## Scope

### In scope

- The ten bilingual DSL guide files listed above and directly conflicting maintained links if required.

### Out of scope

- Runtime behavior, terminal response implementation, new DSL syntax, status remapping, and test changes.

## Acceptance criteria

- All maintained DSL guides agree that response-stage gRPC errors are supported and preserve their exact gRPC code/message.
- No stale request-only, terminate-degradation, or warning-only claim remains.
- HTTP 200 and independent `grpc_status` semantics are stated consistently.
- Existing response-stage unit and integration coverage is cited as the verification floor without modifying it.
- Documentation validation and all applicable repository guards pass.
