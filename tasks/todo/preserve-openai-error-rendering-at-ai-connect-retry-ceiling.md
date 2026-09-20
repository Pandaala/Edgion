# Preserve OpenAI Error Rendering at the AI Connect Retry Ceiling

## Status

Ready for a separate design and implementation task after the route/Pingora
retry-ceiling contract is defined.

## Problem

When AI endpoint-scoped connect failures consume the locked Pingora proxy loop's
final iteration, `fail_to_connect` can advance the scheduler and mark the error
retryable without receiving the next `upstream_peer` callback. The request then
falls through generic terminal handling and may emit an HTML 503 instead of the
OpenAI-compatible `BackendUnavailable` JSON envelope used by normal AI
successor exhaustion.

## Constraints

- Coordinate with `tasks/todo/align-route-retry-limits-with-pingora-loop-ceiling.md`.
- Keep exactly one owner for terminal response rendering; do not write an
  asynchronous local response from the synchronous connect-failure callback.
- Preserve endpoint-scoped service skipping, exactly-once reservation release,
  retry/outlier accounting, canonical access-log diagnostics, and response
  commitment rules.
- Keep internal endpoint, credential, scheduler, and dependency details out of
  client-facing error bodies.
- Do not modify Pingora fork behavior without an explicit cross-repository
  contract and regression tests.

## Acceptance direction

Exercise AI backends through the effective loop boundary and prove that normal
successor exhaustion and ceiling exhaustion produce the same fixed OpenAI error
schema/status while preserving attempt history and exactly-once cleanup.
