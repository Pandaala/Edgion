# Add Native WAF Response-stage Invalid-status Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | test |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add a wire scenario proving that a native-WAF response-stage disruptive action
with `status:100` cannot fall through to the original upstream response.

## Requirements

- Configure a native WAF response-stage disruptive rule with `status:100`.
- Assert a valid fail-closed final response and no original upstream response.
- Assert the canonical access-log terminal status without adding parallel logs.
- Use bounded waits and deterministic cleanup.

## Scope

- Add or extend integration coverage only after explicit authorization for the exact integration paths.
- Do not modify existing `edgion-tests/**` files without explicit path-level authorization.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Schedule a separate repair | Existing integration tests are the frozen validation control line. |
