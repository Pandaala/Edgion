# D0120 — UDP Session Retirement Identity

- Status: pending
- Severity: medium
- Discovered in: B0212 UDP proxy analysis
- Canonical task: [Retire UDP sessions by generation](../../../todo/retire-udp-sessions-by-generation.md)

Expiration collection precedes logging awaits and unconditional key removal;
activity can resume before removal. Old receiver tasks check key existence rather
than generation identity, allowing a recreated key to keep an old task alive.
Coordinate retirement, receiver and permit ownership in a plan-first task.
