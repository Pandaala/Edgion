# Add Downstream Request-Body Timeout 408 Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add a bounded H1/H2 integration scenario proving that a stalled downstream request-body upload reaches the canonical 408 outcome without client-closed or backend-failure attribution.

## Requirements

- Stall an unfinished downstream upload until the configured read timeout expires, without actively closing or resetting the connection first.
- Cover H1 and H2 when the shared topology can distinguish their transport behavior without duplicated setup.
- Assert the wire response and access-log status are 408, `ClientClosed` is absent, and the selected backend receives neither an error attribution nor a local-origin outlier failure.
- Use bounded waits and deterministic cleanup.

## Scope

### In scope

- Existing request-timeout and transport integration infrastructure after explicit authorization for the required integration changes.
- Minimal test-server/client support needed for a deliberately stalled upload.

### Out of scope

- Editing an existing `edgion-tests/**` file without explicit authorization for that test path.
- Changing response buffering, retry policy, metrics, or access-log schema.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Schedule a separate repair | Existing coverage tests active disconnect/reset behavior; the meaningful stalled-upload topology requires changes to frozen integration infrastructure. |
