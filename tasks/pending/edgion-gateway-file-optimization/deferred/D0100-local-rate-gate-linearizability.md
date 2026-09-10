# D0100 — Local Rate-gate Linearizability

- Status: pending
- Severity: high (admission correctness and hot-path contention)
- Discovered in: B0148 local rate-gate analysis
- Canonical task: [Make local rate-gate transitions linearizable](../../../todo/make-local-rate-gate-transitions-linearizable.md)

The refill mutex does not cover the later token-decrement CAS, so a concurrent
refill can restore a token already consumed after its snapshot. Parameter hot
reload can similarly publish mixed state or overwrite a concurrent transition,
while every connection also takes the process-global registry mutex and rewrites
parameters. Resolve these together through a separately reviewed, plan-first
linearizable per-gate design with a cold-path registry; D0028 retains numeric
bounds; [D0099](../../../../skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md)
records the accepted listener scheduling limitation. Existing integration tests remain
frozen.
