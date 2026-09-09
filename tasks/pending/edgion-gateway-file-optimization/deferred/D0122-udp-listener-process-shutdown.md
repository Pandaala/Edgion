# D0122 — UDP Listener Process Shutdown

- Status: pending
- Severity: medium
- Discovered in: B0213 UDP module analysis
- Canonical task: [Wire UDP listener process shutdown](../../../todo/wire-udp-listener-process-shutdown.md)

UDP cancellation registry registration is live, but cancellation has no production
shutdown caller. Auxiliary runtime teardown stops tasks rather than explicit
registry-driven completion. Define shutdown ownership and ordering in a separate
reviewed plan; do not patch in an arbitrary cancel_all call.
