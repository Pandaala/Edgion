# D0127 — Empty Individual Outbound CA Bundles

- Status: pending
- Severity: medium
- Discovered in: B0231 outbound TLS client analysis
- Canonical task: [Reject empty individual outbound CA bundles](../../../todo/reject-empty-individual-outbound-ca-bundles.md)

Three canonical outbound TLS adapters validate certificate yield only across the
aggregate, allowing a valid bundle to mask another declared empty or non-PEM
bundle. Independent review confirmed reachability and P2 correctness impact, not
a trust bypass. Repair is plan-first and cross-adapter; B0231 does not partially
patch one consumer.
