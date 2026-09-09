# Add Streaming Connection-Established Access-Log Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P3 |
| Issue | N/A |

## AI guide summary

Add one integration scenario proving that a long-lived streaming protocol emits the immediate canonical access-log record with `conn_est == true` while the connection remains open.

## Requirements

- Prefer one WebSocket scenario unless the harness proves gRPC-Web materially distinct.
- Observe the configured access-log output used by `AccessLogger::send`; do not assume the integration-only terminal `AccessLogStore` captures the immediate path.
- Assert the record arrives before connection teardown and carries the canonical connection-established marker.
- Preserve bounded waits and deterministic cleanup.

## Scope

### In scope

- A new standalone integration file if it can be discovered without modifying existing `edgion-tests/**` files.
- Otherwise, design and later wire the scenario after explicit authorization for the required harness changes.

### Out of scope

- Editing, deleting, or renaming an existing integration-test file without explicit authorization for that file.
- Changing access-log production behavior or adding a parallel log sink.
- Duplicating both WebSocket and gRPC-Web coverage without evidence.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Schedule a separate repair | Existing traffic integrations do not assert the immediate record, and the frozen test harness may require later wiring changes. |
