# D0101 — Invalid L4 GeoIP Live Fail-close Verification

- Status: pending
- Severity: medium (integration regression coverage)
- Discovered in: B0152 stream GeoIP plugin analysis
- Canonical task: [Verify invalid L4 GeoIP generations fail closed](../../../todo/verify-invalid-l4-geoip-generations-fail-closed.md)

The frozen `parse_only_is_rejected_at_l4` scenario currently stops after the
resource reports `Accepted=False`. A later integration-test-authorized change
should probe live TCP traffic to prove the invalid generation fails closed, then
publish a valid generation and prove recovery. The current campaign must not edit
the integration test.
