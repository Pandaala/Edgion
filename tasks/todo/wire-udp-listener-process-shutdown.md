# Wire UDP Listener Process Shutdown

## Meta

- Status: pending
- Type: bug
- Priority: P2
- Severity: medium
- Source: Gateway campaign B0213 / D0122

## Evidence and Acceptance

`routes/udp/mod.rs` registers listener tokens through listener construction, but
neither cancel nor cancel_all has a production caller. UDP serve tasks run on the
auxiliary runtime, outside Pingora Service management. The shutdown watch sent
after Pingora returns is not bridged to UDP tokens; remaining runtime lifetime
ends in teardown rather than explicit UDP completion. No resource leak after
process exit is claimed.

- Plan first across the registry, CLI/listener startup ownership and only necessary
  UDP task cancellation/log coordination. Choose and document whether listeners
  stop at Pingora shutdown start or at post-Pingora auxiliary drain.
- Deliver real shutdown to every registered listener and join/otherwise account
  for serve, cleanup and receiver completion; repeated shutdown is idempotent.
- Define token/task ownership instead of treating runtime destruction as graceful
  completion. Coordinate session terminal statistics/permits/sockets with D0120.
- Preserve frozen-at-startup ports; do not add dynamic listener lifecycle features.
- Add deterministic token/registry tests in the later fix. New shutdown-with-UDP
  wire coverage remains a deferred integration requirement; frozen tests stay unchanged.
