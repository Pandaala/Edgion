# D0105 — Response Content-Encoding Classification

- Status: pending
- Severity: medium (response policy correctness)
- Discovered in: B0187 HTTP response-head analysis
- Canonical task: [Unify response Content-Encoding classification](../../../todo/unify-response-content-encoding-classification.md)

Guardrail admission and final response-head gating currently inspect only the
first `Content-Encoding` field. A first `identity` field followed by another
coding, or a comma-delimited mixed value, can be misclassified. The correction
requires one borrowed all-field/all-token classifier shared by the plugin-stage
session and final-head paths; a target-local response-head patch would leave the
two gates inconsistent.
