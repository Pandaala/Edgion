# Correct Gateway API RequestRedirect Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | docs |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Align the English and Chinese Gateway API RequestRedirect guides with the supported status-code set and terminal redirect rule shape.

## Requirements

- Document the supported `301`, `302`, `303`, `307`, and `308` redirect status codes accurately.
- Remove or correct examples that combine terminal RequestRedirect behavior with `backendRefs`.
- Preserve query, path replacement, hostname, scheme, and port descriptions that match the current runtime and Gateway API contract.
- Keep English and Chinese structure, examples, and semantics synchronized.
- Do not modify existing integration-test files.

## Acceptance criteria

- Both guides present only valid route shapes and the same supported status values.
- Documentation validation passes and relevant links resolve.
- Existing redirect unit tests remain unchanged unless a doc claim requires a focused assertion.
