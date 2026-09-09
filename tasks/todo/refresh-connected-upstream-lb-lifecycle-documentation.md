# Refresh Connected-Upstream Load-Balancer Lifecycle Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | docs |
| Priority | P3 |
| Issue | N/A |

## AI guide summary

Update Pingora lifecycle architecture to describe the current attempt-owned load-balancer commit guard used by both LeastConn and EWMA instead of the removed direct runtime increment path.

## Requirements

- Replace stale LeastConn-only and `runtime_state::increment()` wording with `lb::commit_attempt` and `LbCommitGuard` ownership.
- Describe retry transition, cancellation, terminal cleanup, and idempotent commit behavior.
- Keep connection establishment timing distinct from EWMA latency sampling at the final response-header stage.
- State that Phase 1 initializes the gRPC-Web bridge before Pingora's module request-header filter, and move protocol detection/request-info extraction to the later `request_filter` phase.
- Verify HTTP and gRPC consumers against current code before editing the documentation.

## Scope

### In scope

- `skills/01-architecture/02-gateway/02-pingora-lifecycle.md`.
- Source-only verification of current upstream attempt and response-header accounting.

### Out of scope

- Runtime load-balancer, retry, metrics, or callback changes.
- Changes to existing `edgion-tests/**` files.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Schedule a separate repair | The callback is correct; only its architecture description is stale. |
