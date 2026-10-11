# Verify native WAF admission and reload on Kubernetes

Status: Pending cluster validation; P2

Local Controller-to-Gateway wiring and supported-profile fixtures are verified. Validate the same path against a real Kubernetes API server when a test cluster is available.

- Check that the deployed CRD preserves configured WAF fields and the pinned supported bundle/policy resources.
- Exercise standard/strict, On/DetectionOnly, Selector updates, and expensiveChecks omitted/false/true.
- Verify invalid-generation retention, in-flight generation pins, and JSON/XML attack verdicts through Gateway routing.
- Record cluster/CRD/source versions and exact outcomes; clean up only test-owned resources.
- This is verification of current features, not authorization for new schema or response-body inspection.

Reuse `config/waf/crs-4.25.1-supported-v1/` and the existing WafDynamic/WafExpensiveChecks integration scenarios.
