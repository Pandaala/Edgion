# D0115 — TLSRoute Stage-1 Fallback Removal

- Status: pending
- Severity: medium
- Discovered in: B0205 TLS shared-module analysis
- Canonical task: [Remove TLSRoute Stage-1 fallback coherently](../../../todo/remove-tlsroute-stage1-fallback-coherently.md)

Architecture identifies the fallback as pre-release compatibility debt, but
existing TLSRoute integration fixtures still depend on Stage-1-only plugins.
Removing runtime fallback alone would turn their deny behavior into allow.
Coordinate contract and fixture migration outside the frozen-test campaign.
