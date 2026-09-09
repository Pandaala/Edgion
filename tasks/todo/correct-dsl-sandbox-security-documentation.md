# Correct DSL Sandbox Security Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | docs |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Align the English and Chinese DSL sandbox security guides with the fixed panic diagnostic and the distinct outbound-response versus request-body failure contracts.

## Requirements

- Update both DSL sandbox security pages:
  - `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/sandbox-security.md`
  - `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/dsl/sandbox-security.md`
- State that `execute_safe` discards the panic payload and records only the fixed `vm panic` classification; do not claim that panic messages, script names, or trigger details are logged.
- Distinguish outbound `http.call` response-stream failure, which returns the documented atomic failure map, from request-body upload/read failure, which enters the fixed `BodyAccess` fail-close path.
- Verify all nearby error-policy and observability statements against current runtime code and maintained DSL knowledge before editing.
- Keep the English and Chinese pages semantically equivalent. Do not change runtime behavior, add logs, or expose diagnostic detail.

## Scope

### In scope

- The two sandbox security pages and directly necessary documentation links.

### Out of scope

- DSL runtime changes, error-policy redesign, new observability, or integration tests.

## Acceptance criteria

- Both pages accurately describe panic isolation and `http.call`/body-access failures without promising unavailable or sensitive diagnostics.
- Documentation validation and all applicable repository guards pass.
