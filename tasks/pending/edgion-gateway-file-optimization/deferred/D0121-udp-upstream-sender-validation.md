# D0121 — UDP Upstream Sender Validation

- Status: pending
- Severity: medium
- Discovered in: B0212 UDP proxy analysis
- Canonical task: [Validate UDP upstream response source](../../../todo/validate-udp-upstream-response-source.md)

The bound, unconnected upstream socket discards recv_from's source address and
forwards received bytes. Destination validation does not authenticate a received
datagram's sender. Define the pinned-source contract before a coordinated fix.
