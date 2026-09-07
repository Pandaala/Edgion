# D0089 — Fail Closed on Missing Gateway API ExtensionRef Configuration

## Meta

- Status: needs-implementation
- Severity: high
- Discovered in: B0126 Gateway API ExtensionRef and module-root analysis
- Category: schema-validation
- Canonical task: [`tasks/todo/fail-closed-missing-gateway-api-extensionref-configuration.md`](../../../todo/fail-closed-missing-gateway-api-extensionref-configuration.md)

## Evidence

HTTP and GRPC route filters can deserialize `type: ExtensionRef` with an absent optional `extensionRef` payload outside Kubernetes. HTTP Controller validation returns early for the missing value, GRPC validation checks only present values, and both Gateway builder arms use `if let Some`, so the malformed filter silently disappears. Kubernetes admission alone rejects this pairing through vendored CEL. If the omitted reference was intended to carry authentication or authorization, filesystem or etcd ingestion can therefore fail open.

## Why deferred

Correct closure requires one shared type/payload validation decision, HTTP and GRPC Controller invalid-rule/status handling at rule and backendRef scope, a Gateway construction backstop, and focused tests. That cross-crate behavior change does not belong in B0126's local immutable-target optimization.

## Integration-test impact

Shared-resource, Controller-status, and Gateway-runtime tests should prove deserialization, `PartiallyInvalid`, and the 500 backstop. Add a new standalone non-Kubernetes scenario only if those tests cannot faithfully prove the ingest boundary; do not modify existing frozen integration files.
