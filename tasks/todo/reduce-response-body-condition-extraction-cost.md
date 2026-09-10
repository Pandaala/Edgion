# Reduce Per-chunk Response-body Condition Extraction Cost

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-07 |
| Status | pending |
| Type | performance-design |
| Priority | P3 |
| Issue | N/A |
| Depends on | Canonical condition evaluator and [response-body no-work-gate decision](../../skills/04-review/cpu-memory/intentional-design/async-response-body-hook-profile-before-gate.md) |

## AI guide summary

Design a semantics-safe way to avoid repeated owned extraction work when response-body conditions are evaluated for every upstream chunk.

## Requirements

- Inventory every condition source used by response-body plugins and measure which paths allocate, parse JSON, or clone certificate data per chunk.
- Preserve dynamic time and probability evaluation and every stage-visible request-context mutation semantic.
- Prefer borrowed extraction or narrowly classified stable stage inputs; do not introduce a second predicate model or cache implementation.
- Preserve DirectPeerIp/RemoteIp trust boundaries, canonical KeyGet/session ownership, redaction, streaming/backpressure, and bounded memory.
- Coordinate with the permanent
  [response-body no-work-gate decision](../../skills/04-review/cpu-memory/intentional-design/async-response-body-hook-profile-before-gate.md)
  without duplicating its outer dispatch/future scope.
- Do not modify existing integration-test files.

## Acceptance criteria

- The design identifies exactly which sources may be borrowed or cached and proves why their values cannot change within the chosen lifetime.
- Dynamic predicates and mutable context sources retain current per-evaluation behavior.
- Focused benchmarks or allocation-sensitive unit evidence demonstrate the hot-path benefit.
- If the final design changes externally observable timing or stage semantics, create a separate integration-coverage issue before implementation; otherwise no integration issue is required.
