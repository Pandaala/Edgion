# D0118 — Malformed ClientHello Wire Coverage

- Status: covered
- Severity: low (integration-test gap)
- Discovered in: B0210 parser analysis
- Canonical knowledge: [ClientHello read contract](../../../../skills/04-review/tls/regression-guards/sni-passthrough-clienthello-read.md)

The authorized follow-up added an isolated Cargo socket integration matrix in
`edgion-gateway/tests/tls_passthrough_clienthello.rs`. The canonical knowledge page
owns its verification, scope, and limits. Existing integration files and production
parser behavior remain unchanged.
