# D0085 — Complete Native Gateway API CORS Validation Parity

## Meta

- Status: needs-implementation
- Severity: medium
- Discovered in: B0124 Gateway API CORS filter analysis
- Category: schema-validation
- Canonical task: [`tasks/todo/complete-native-gateway-api-cors-validation-parity.md`](../../../todo/complete-native-gateway-api-cors-validation-parity.md)

## Evidence

`HTTPCORSFilter::validate` checks only part of origin syntax. It does not enforce the vendored Gateway API v1.5 method enum, header-name grammar, scheme and port constraints, list bounds and set semantics, wildcard exclusivity, or the `maxAge` minimum and default. Filesystem and etcd inputs can therefore publish values that Kubernetes admission would reject. D0040 explicitly owns EdgionPlugins CORS rather than this native Gateway API type.

## Why deferred

The correction belongs to shared resource validation, Controller invalid-rule/status handling, Gateway construction, and their tests. The accepted non-Kubernetes schema-validation asymmetry requires complete handler validation rather than injecting full OpenAPI admission into watcher paths.

## Integration-test impact

Shared resource, Controller, and Gateway tests should prove validation and default parity. The canonical task may add a standalone non-Kubernetes scenario if those tests cannot prove the ingest boundary; no separate integration issue is required now.
