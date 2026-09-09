# Retire UDP Sessions by Generation

## Meta

- Status: pending
- Type: bug
- Priority: P2
- Severity: medium
- Source: Gateway campaign B0212 / D0120

## Evidence and Scope

In `routes/udp/edgion_udp.rs`, cleanup collects expired session Arcs, awaits
terminal logging, then removes by key without revalidating activity/identity.
Send-result paths can restore activity in that interval. Upstream receivers only
check key existence, so a new session under the same key can keep an old receiver,
socket and buffer alive after its permit was released. This is a source-proven
race, not a reproduced network exploit or measured unbounded resource growth.

Use a reviewed plan for UDP admission, retirement and receiver coordination,
necessary local test seams and owning knowledge. Preserve protocol behavior.

- Define one identity-aware expiration/retirement boundary, including resumed
  activity after candidate collection. Never retire a replacement generation.
- Old receivers must stop independently of a new same-key session's presence.
- Couple permits to the actual bounded resource lifetime, including receiver
  errors and cancellation; identify socket/task ownership explicitly.
- Emit terminal logs only for retired sessions; logging awaits must not widen
  the admission/retirement race. Add no parallel traffic tracing.
- Use controlled scheduling for expiry/activity, remove/recreate and receiver
  failure/cancellation. Defer new wire scenarios; do not edit frozen integration.

B0212's proposed inline counters and delayed IP formatting do not resolve this
issue and can proceed without changing these lifecycle semantics.
