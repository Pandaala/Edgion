# D0004 — Make Health Task/Status Generations Cancellation-safe

## Meta

- Status: needs-discussion
- Severity: high
- Discovered in: B0040, `edgion-gateway/src/backends/health/check/`
- Category: correctness

## Evidence

Health-check task replacement and removal call `JoinHandle::abort()` and immediately update or unregister the shared status store. Abort is not a synchronous join. An old loop that is executing synchronous work can still register its Service or publish a probe result after the new generation has started or after removal completed, then reach its next cancellation point. That can recreate cache residue for a deleted Service or let old and new task generations update the same threshold state.

The existing displaced-handle fix guarantees that every replaced handle is aborted. It does not establish ownership of later status writes by the current task generation.

## Why deferred

A safe correction requires one generation/cancellation contract across task replacement, task shutdown, status registration, result publication, and policy reconciliation. A local abort reorder or extra unregister can still race with an old writer.

## Impact and open questions

- A removed Service can regain health-store state after unregister completes.
- Old-generation results can influence the replacement generation's thresholds.
- Define whether publication checks an epoch token or task replacement awaits cancellation completion.
- Coordinate with the existing atomic Service-policy snapshot work without merging their ownership contracts.
- Preserve bounded probe concurrency, startup jitter, ticker cadence, and lock-free selector reads.

## Candidate next step

Design a generation token or cancellation-completion protocol. Add deterministic tests that pause an old loop immediately before registration and result publication, replace or remove its configuration, resume it, and prove no old-generation Service or backend status survives.
