# Isolate Shared Integration Process Lifecycle

## Meta

- Status: pending
- Type: bugfix
- Priority: P2
- Discovered during: add-l4-duplex-backpressure-integration

## Evidence and scope

The canonical local integration topology uses fixed host ports across worktrees.
`edgion-tests/integration/scripts/utils/start_all_with_conf.sh` also invokes
`pkill -9 -f "debug/edgion-controller"`, matching processes outside its worktree;
equivalent patterns target Gateway and test_server. Independent worktree runs
can therefore kill each other's live test topology rather than fail as busy.

On 2026-09-09, ws7's 15:29:43 run lost its shared services when ws4 started at
15:30:21. A second ws7 attempt at 15:31:25 explicitly logged `Killed: 9` for
test_server, Controller, and Gateway while ws4 started at 15:31:34. The L4 suite
reported fixture cleanup failure and connection refusal; these are infrastructure
interruptions, not evidence of a relay defect.

## Acceptance criteria

- Coordinate ownership of shared host ports across worktrees before startup or
  cleanup; a conflicting run waits with a deadline or fails clearly as busy.
- Stop only processes proven to belong to the current run. Handle stale owner
  records and PID reuse without signalling unrelated processes.
- Preserve the canonical shared topology and existing scenario registrations.
- Test overlapping starts, owner failure, cleanup, and stale ownership on macOS
  and Linux. Keep this runner change separate from the L4 test implementation.
