# D0092 — Complete Gateway API Request-Transform Validation and Preparation Parity

## Meta

- Status: needs-implementation
- Severity: high
- Discovered in: B0128 Gateway API redirect/rewrite analysis
- Category: schema-validation
- Canonical task: [`tasks/todo/complete-gateway-api-request-transform-validation-and-preparation-parity.md`](../../../todo/complete-gateway-api-request-transform-validation-and-preparation-parity.md)

## Evidence

RequestRedirect and URLRewrite accept structurally different native HTTPRoute, delegated/effective, and EdgionPlugins inputs across Kubernetes, filesystem, and etcd ingestion. Type/config pairing, path-modifier shape, status/scheme/hostname/port bounds, repeated-filter cardinality, and runtime preparation are not enforced through one Controller/Gateway decision, so malformed non-admission input may publish or fail only on traffic.

## Why deferred

Correct closure spans shared resource validation, Controller invalid-rule/status handling, every rule/backend filter list, Gateway defensive construction, and immutable typed preparation. B0128 only corrects the shared runtime prefix transformation and canonical downstream-scheme selection.

## Integration-test impact

Prefer shared-resource, Controller-status, and Gateway-runtime unit tests. Create a standalone non-Kubernetes integration issue only if those tests cannot prove the ingest boundary; do not modify existing frozen integration files.
