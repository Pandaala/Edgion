# D0118 — Malformed ClientHello Wire Coverage

- Status: pending
- Severity: low (integration-test gap)
- Discovered in: B0210 parser analysis
- Canonical task: [Add malformed ClientHello passthrough coverage](../../../todo/add-malformed-clienthello-passthrough-coverage.md)

B0210 proposes unit-level nested-length rejection within the existing bounded
single-record parser. Later wire coverage should verify malformed complete
records close without backend relay while valid and trailing-byte traffic remains
unchanged. Existing integration files are frozen; no new integration is added now.
