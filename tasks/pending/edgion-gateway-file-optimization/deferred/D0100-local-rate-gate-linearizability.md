# D0100 — Local Rate-gate Linearizability

- Status: done by external commit `d43d2539` (2026-09-10)
- Severity: high (admission correctness and hot-path contention)
- Discovered in: B0148 local rate-gate analysis
- Canonical task: completed and removed by the external repair

The refill mutex does not cover the later token-decrement CAS, so a concurrent
refill can restore a token already consumed after its snapshot. Parameter hot
reload can similarly publish mixed state or overwrite a concurrent transition,
while every connection also takes the process-global registry mutex and rewrites
parameters. Resolve these together through a separately reviewed, plan-first
linearizable per-gate design with a cold-path registry; D0028 retains numeric
bounds; [D0099](../../../../skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md)
records the accepted listener scheduling limitation. Existing integration tests remain
frozen.

External commit `d43d2539` introduced the separately reviewed linearizable local
connection gate and its regression knowledge, then removed the canonical task.
The affected Gateway files were reopened for campaign coverage rather than the
completed task being recreated.
