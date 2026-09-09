# D0125 — TLS Collision Snapshot Retention

- Status: needs-investigation
- Severity: low
- Discovered in: B0222 production TLS matching analysis
- Canonical task: [Investigate Gateway TLS collision snapshot retention](../../../todo/investigate-gateway-tls-collision-snapshot-retention.md)

Published candidate vectors retain compiled losers while lookup selects the
winner. Practical memory value is unmeasured; investigate before plan-first
implementation, without classifying this as a security defect.
