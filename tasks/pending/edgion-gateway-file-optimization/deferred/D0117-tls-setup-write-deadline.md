# D0117 — TLS Setup Write Deadline

- Status: needs-investigation
- Severity: low
- Discovered in: B0206 passthrough analysis
- Canonical task: [Investigate TLS setup write deadlines](../../../todo/investigate-tls-setup-write-deadlines.md)

PP2 and initial ClientHello write/flush calls precede the shared relay idle
clock and lack a local timeout wrapper. Their payload is bounded; pinned
transport timeout defaults and actual blocking conditions remain unverified.
Do not classify this as a proven unbounded hang or denial of service.
