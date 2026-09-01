# Isolate RequestMirror Unit Tests from the Network

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | test-isolation |
| Priority | P2 |
| Issue | N/A |
| Depends on | B0102 |

## AI guide summary

Replace RequestMirror unit-test network attempts with a deterministic no-dial arming seam while retaining the existing behavioral assertions.

## Requirements

- Inventory the four arming tests and the detached writer entry points they trigger.
- Add the smallest test-only decision or writer seam that prevents DNS/socket activity without bypassing production behavior.
- Preserve assertions for sampling, endpoint selection, semaphore acquisition, body snapshot/tee ownership, cancellation, and access-log state.
- Do not delete coverage or redirect unit tests to a real local/external server.
- Leave existing integration tests unchanged.

## Acceptance criteria

- RequestMirror unit tests make no DNS or socket attempts.
- Existing arming and body-state assertions remain deterministic.
- Focused tests pass repeatedly without background-task leakage.
- Workspace format/check/Clippy and repository guards pass.
