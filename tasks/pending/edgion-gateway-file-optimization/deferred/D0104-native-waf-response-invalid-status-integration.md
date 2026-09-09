# D0104 — Native WAF Response Invalid-status Integration

- Status: resolved (2026-09-09)
- Severity: medium (integration regression coverage)
- Discovered in: B0186 HTTP response-filter analysis
- Maintained knowledge: [Response-stage denial must terminate](../../../../skills/04-review/http-body/regression-guards/response-stage-denial-must-terminate.md)

A later integration-test-authorized change should add a native-WAF response-stage
scenario with disruptive `status:100`. Assert a valid fail-closed final response,
no upstream-response fall-through, and the canonical access-log status. B0186
must not modify the existing frozen integration suite.

Resolved by the path-isolated `WafDynamic` phase-3 wire case. The authorized
follow-up verifies the fail-closed 500 response, absence of upstream body
markers, preserved upstream 200 observation, and the exact native-WAF
intervention in the canonical access log.
