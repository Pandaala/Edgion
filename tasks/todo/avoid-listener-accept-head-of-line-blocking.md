# Avoid Listener Accept Head-of-line Blocking

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | performance / availability |
| Priority | P1 |

## Problem

Pingora defaults to one listener task per file descriptor. That task awaits the
connection filter before it spawns the accepted connection, while Edgion's
Stage 1 stream-plugin bridge may await Redis-backed rate-limit decisions. A
slow dependency can therefore serialize later accepts on the same listener
file descriptor and fill the kernel backlog even when the plugin ultimately
fails open.

## Requirements

- Reproduce the accept-loop serialization with a deterministic slow connection
  filter and prove that a later socket cannot currently be admitted independently.
- Design the fix before implementation. Do not use unbounded task spawning or
  timeout reduction as a substitute for bounded admission ownership.
- Ensure dependency awaits do not occupy the sole accept loop while preserving
  the fatal PROXY-policy guard, pre-PROXY `DirectPeerIp`, pre-TLS denial, plugin
  order and configured failure policy.
- Bound concurrent admissions and accepted file descriptors; define overload,
  cancellation, listener shutdown and task-failure behavior.
- Evaluate Pingora's `listener_tasks_per_fd`, reuse-port behavior and whether the
  correct solution belongs in the Edgion Pingora fork or the Gateway listener
  composition layer.
- Preserve immutable stream-plugin snapshot pinning for each admission decision
  and existing bounded metrics; do not add per-connection tracing.
- Add deterministic unit or component coverage for slow filters, saturation and
  shutdown. Keep existing integration-test files unchanged; propose separate
  integration coverage only if the final design needs it.
