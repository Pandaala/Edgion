# D0110 — L4 Duplex Backpressure Integration

- Status: pending
- Severity: medium
- Discovered in: B0196 L4 duplex analysis
- Canonical task: [Add L4 duplex backpressure integration coverage](../../../todo/add-l4-duplex-backpressure-integration.md)

Existing TCP/TLS integration coverage exercises small exchanges and idle closure,
not reverse progress during a stalled directional write or flush. Schedule the
shared-topology scenarios separately; existing integration files remain frozen.
