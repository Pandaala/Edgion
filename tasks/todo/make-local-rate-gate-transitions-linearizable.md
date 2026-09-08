# Make Local Rate-gate Transitions Linearizable

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | correctness / performance |
| Priority | P1 |

## Requirements

- Design before implementation. Make elapsed refill and one-token consumption a
  single linearizable admission transition; a refill must not restore a token
  consumed after its snapshot.
- Publish rate, interval, tokens and refill timestamp coherently. Define capacity
  increase/decrease and overlapping old/new runtime-generation semantics without
  mixed parameters or overwriting concurrent admission.
- Remove the process-global `LOCAL_GATES` mutex and unconditional parameter
  update from every connection hit. Reuse or pin the entry-scope plus listener-
  port gate through a bounded or sharded design without adding hit-path allocation.
- Preserve `<namespace>/<name>/#index` plus port identity, state continuity across
  hot reload, layered attachment behavior and the accepted no-eviction policy.
- Add controlled-clock deterministic concurrency tests for a lost decrement,
  exact concurrent capacity, configuration-update races, reversed samples and
  the zero timestamp sentinel.
- Preserve the monotonic clock, Stage ordering, metrics, failure policy, Redis
  behavior and `DirectPeerIp` semantics.
- Coordinate but do not merge scopes: D0028 owns numeric bounds and overflow;
  D0099 owns listener accept-loop head-of-line blocking around dependency awaits.
- Keep existing integration-test files unchanged. Add an integration-test issue
  separately only if the approved design introduces behavior not covered by
  deterministic Gateway tests.
