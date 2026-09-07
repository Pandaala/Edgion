# D0090 — Complete Gateway API Header-Modifier Validation and Preparation Parity

## Meta

- Status: needs-implementation
- Severity: medium
- Discovered in: B0127 Gateway API header-modifier analysis
- Category: schema-validation
- Canonical task: [`tasks/todo/complete-gateway-api-header-modifier-validation-and-preparation-parity.md`](../../../todo/complete-gateway-api-header-modifier-validation-and-preparation-parity.md)

## Evidence

HTTP and GRPC native header modifiers retain raw configured names and values until request execution. Validation is incomplete across native route filters, backend filters, delegated rules, and the preserve-unknown-fields EdgionPlugins path, so invalid lists can publish and fail only on traffic. Existing CORS, RateLimit, RealIp, and response-mutation tasks do not own this complete filter contract.

## Why deferred

Correct closure spans shared resources, Controller status, Gateway defensive construction, HTTP and GRPC rule/backend filter lists, typed immutable preparation, and bilingual documentation. B0127 only corrects first-equivalent entry semantics without broadening into cross-crate validation or framing policy.

## Integration-test impact

Prefer shared-resource, Controller-status, and Gateway-runtime unit tests. Create a standalone integration issue only if those tests cannot prove the ingest and publication boundary; do not modify existing frozen integration files.
