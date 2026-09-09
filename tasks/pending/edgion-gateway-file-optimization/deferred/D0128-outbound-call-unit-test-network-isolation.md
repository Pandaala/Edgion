# D0128 — Outbound Call Unit-Test Network Isolation

- Status: pending
- Severity: low
- Discovered in: B0245 outbound call analysis
- Canonical task: [Isolate shared outbound-call unit tests from network](../../../todo/isolate-shared-outbound-call-unit-tests-from-network.md)

The shared outbound-call library unit-test module contains a broad MockServer
matrix that opens loopback sockets. The behavior is load-bearing, so deleting or
weakening cases is not acceptable. The standalone task owns deterministic
transport/clock seams or an explicit non-frozen test target; B0245 adds only
network-free coverage for its local ownership and timing changes.
