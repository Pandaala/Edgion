---
name: review-testing
description: Review knowledge for testing, grouped by decision type.
---

# testing review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [Dynamic Secret fixtures must be tracked](mandatory-design/dynamic-secret-fixtures-must-be-tracked.md)

## Intentional design

- [ACME GatewayRef Isolation Coverage Is Not a Standalone Defect — WONT-FIX](intentional-design/acme-gatewayref-isolation-coverage-not-standalone.md)
- [Kubernetes integration cleanup requires run-scoped ownership](intentional-design/kubernetes-integration-cleanup-ownership-boundary.md)
- [Local shared integration topology allows one runner and global cleanup](intentional-design/local-shared-topology-single-owner-takeover.md)
- [`/api/v1/debug/store-stats` path prefix vs testing-mode gate — ACCEPTED-TRADEOFF](intentional-design/store-stats-testing-router-naming.md)
- [Testing-router error-body enumeration & no-auth on `create_testing_router` — ACCEPTED-TRADEOFF](intentional-design/testing-router-error-body-enumeration.md)

## Regression guards

- [Kubernetes dynamic scenario inventory parity](regression-guards/kubernetes-dynamic-scenario-inventory-parity.md)
- [Sensitive-mode tests use exact subprocess isolation](regression-guards/sensitive-mode-test-subprocess-isolation.md)
- [Kubernetes preflight accepts kubectl Node collections](regression-guards/kubernetes-preflight-node-collection.md)
- [Adaptive telemetry reconciliation needs destructive-set coverage](regression-guards/ai-adaptive-reconcile-needs-destructive-set-coverage.md)
- [AI streaming client disconnect needs wire coverage](regression-guards/ai-stream-client-disconnect-needs-wire-coverage.md)
- [AI tests must discriminate the claimed boundary](regression-guards/ai-tests-must-discriminate-the-claimed-boundary.md)
- [AI time-boundary tests use controlled clocks](regression-guards/ai-time-boundary-tests-use-controlled-clocks.md)
- [Body suite sweep registration](regression-guards/body-suite-sweep-registration.md)
- [Dynamic scenario enumeration must fail closed](regression-guards/dynamic-scenario-enumeration-must-fail-closed.md)
- [ExtProc Conformance Evidence — FIXED BASELINE](regression-guards/ext-proc-conformance-evidence.md)
- [Integration builds end before runtime](regression-guards/integration-builds-end-before-runtime.md)
- [Integration LB metrics require scoped deltas](regression-guards/integration-lb-metrics-require-scoped-deltas.md)
- [Integration orchestrators need owned deadlines](regression-guards/integration-orchestrators-need-owned-deadlines.md)
- [OTLP integration discriminates masking and runtime health transitions](regression-guards/otlp-masking-and-runtime-health-integration.md)
- [Integration port selection must fail closed](regression-guards/integration-port-selection-must-fail-closed.md)
- [Integration README CLI must match the runner](regression-guards/integration-readme-cli-must-match-runner.md)
- [Integration reports must cover the whole run](regression-guards/integration-report-must-cover-the-whole-run.md)
- [Integration suite lifecycle errors must fail](regression-guards/integration-suite-lifecycle-errors-must-fail.md)
- [Integration suite registrations must be checked per sweep — FIXED](regression-guards/integration-suite-registrations-must-be-checked-per-sweep.md)
- [Mandatory integration gates fail closed](regression-guards/mandatory-integration-gates-fail-closed.md)
- [Request restriction resolved conditions coverage](regression-guards/request-restriction-resolved-conditions-coverage.md)
- [Redis TLS container reuse follows current-run certificate identity](regression-guards/redis-tls-container-reuse-certificate-identity.md)
- [Resource diff must fail on incomplete reads](regression-guards/resource-diff-must-fail-on-incomplete-reads.md)
