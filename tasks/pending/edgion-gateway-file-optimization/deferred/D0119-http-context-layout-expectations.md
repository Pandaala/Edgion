# D0119 — HTTP Context Layout Expectations

- Status: pending
- Severity: low (stale regression expectations)
- Discovered in: B0207 broader context verification
- Canonical task: [Reconcile HTTP context layout expectations](../../../todo/reconcile-http-context-layout-expectations.md)

Independent diagnosis reproduced three fixed-size failures caused by committed
HTTP context changes predating B0207. Retain the failed verification record and
reconcile documented costs before updating expectations; do not suppress budgets
or allocation checks. B0207's function-only TLS changes may safely continue.
