# D0145 — DSL sandbox unit-test network isolation

- Status: pending; incidental B0278 analysis finding.
- Canonical standalone task: [isolate sandbox unit tests](../../../todo/isolate-dsl-sandbox-unit-tests-from-network.md).
- Two sandbox library tests start real loopback servers. Preserve map-limit contracts without unit network IO; any required integration addition remains a separate deferred task under user instructions.
- Does not establish a production defect or block runtime_errors.rs review.
