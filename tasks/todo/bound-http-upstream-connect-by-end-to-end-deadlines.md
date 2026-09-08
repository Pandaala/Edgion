# Bound HTTP Upstream Connect by End-to-End Deadlines

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Clamp upstream connection establishment by the remaining route request and client gRPC deadlines so the Gateway stops waiting when the caller's end-to-end budget expires.

## Requirements

- Resolve the connection-establishment bound as the minimum of the configured connect timeout, remaining route request deadline, and remaining client gRPC deadline.
- Preserve request-before-gRPC attribution when both deadlines expire.
- Keep `backendRequest` independent because it begins with backend request I/O rather than connection establishment.
- Preserve canonical timeout diagnostics and HTTP/gRPC terminal status mapping.
- Add bounded shared-topology integration coverage for a stalled connect or TLS handshake after the campaign freeze.

## Scope

### In scope

- HTTP upstream peer timeout configuration and its typed deadline ownership.
- Focused unit tests plus one later end-to-end timeout scenario.

### Out of scope

- Editing an existing `edgion-tests/**` file during the quality campaign.
- Changing retry counts, retry budgets, backend-request timeouts, body replay, or failure policy.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Defer from B0181 | Correct terminal outcome now; connection-time clamping spans upstream-peer timeout configuration and frozen shared-topology integration coverage. |
