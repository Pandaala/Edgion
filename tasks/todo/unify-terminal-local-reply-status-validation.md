# Unify Terminal Local-reply Status Validation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define and enforce one canonical terminal local-reply status contract across
native WAF, plugin schemas, runtime producers, and request/response consumers.

## Requirements

- Inventory every configured or dynamic status used for a terminal local reply.
- Restrict terminal replies to 200–599 and keep transport-confirmed 101 separate.
- Ensure a failed final-writer claim cannot silently resume proxying.
- Preserve nonretryable control-flow ownership and avoid backend outlier attribution.
- Update validation, focused unit tests, and user documentation atomically.

## Out of scope

- Changing ordinary forwarded informational-response behavior.
- Partial producer-only validation that leaves another consumer fail-open.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Schedule a separate repair | The complete invariant crosses resource schemas, WAF compilation, request/response runtime producers, and multiple terminal writers. |
