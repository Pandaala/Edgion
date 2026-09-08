# D0104 — Native WAF Response Invalid-status Integration

- Status: pending
- Severity: medium (integration regression coverage)
- Discovered in: B0186 HTTP response-filter analysis
- Canonical task: [Add native WAF response-stage invalid-status integration coverage](../../../todo/add-native-waf-response-invalid-status-integration.md)

A later integration-test-authorized change should add a native-WAF response-stage
scenario with disruptive `status:100`. Assert a valid fail-closed final response,
no upstream-response fall-through, and the canonical access-log status. B0186
must not modify the existing frozen integration suite.
