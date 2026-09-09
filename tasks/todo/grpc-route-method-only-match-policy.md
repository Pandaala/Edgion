# Define GRPCRoute Method-Only Match Policy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Resolve the inconsistent treatment of a GRPCRoute match that specifies a method without a service. The Gateway currently skips this shape during match-engine construction, while Controller delegation code treats method-only matches as meaningful, so the policy must be decided and enforced consistently from validation and status through published runtime behavior.

## Requirements

- Verify method-only semantics against the authoritative Gateway API version and the exact CRD/defaulting rules vendored by Edgion; do not infer validity from either the Gateway warning or current Controller behavior.
- Inventory Kubernetes admission, File/etcd deserialization, resource validation, Controller status production, gRPC delegation flattening and inherited scope, config publication, `GrpcMatchInfo` compilation, and `GrpcMatchEngine` classification.
- If method-only matching is valid, add a canonical compiled representation and precedence behavior that matches the method across any service, including Exact and RegularExpression forms where permitted.
- If method-only matching is invalid, reject it through canonical validation and produce the appropriate Controller status before publication; retain only a defensive Gateway failure path for impossible or stale input.
- Apply the selected policy atomically across validation, status, delegation, configuration, and Gateway consumption. Do not land a Gateway-only bucket or warning change.
- Preserve valid service-only, service-and-method, header-only, and catch-all matches, along with delegation scope inheritance and gRPC-to-HTTP fallback.
- Reuse shared validation and error classification. Do not introduce route-family-specific policy copies or a new failure-policy shape.
- Diagnostics must follow binary ownership, remain bounded and redacted, and avoid raw patterns, headers, resource payloads, or request data. Do not add request-path tracing, parallel logs, or metrics.
- Add deterministic focused tests for admitted and rejected shapes, defaulted and explicit fields, delegation inheritance, publication/status behavior, Exact and RegularExpression policy, and runtime selection or defensive rejection.
- Keep existing `edgion-tests/**` files frozen unless their exact paths are explicitly authorized; record any necessary live configuration-source scenario for separately authorized work.

## Scope

### In scope

- One explicit method-only policy across resource, Controller, delegation, publication, and Gateway layers.
- Canonical validation/status behavior and focused in-crate regression coverage.
- A separately authorized standalone integration scenario if configuration-source parity cannot be proven in unit tests.

### Out of scope

- General GRPCRoute precedence redesign, unrelated delegation behavior, backend policy, retries, timeouts, or protocol handling.
- New logging, metrics, or observability surfaces.
- Existing integration-test modifications without explicit later authorization.

## Acceptance criteria

- Kubernetes and File/etcd inputs receive the same intentional method-only policy after their respective defaulting paths.
- Controller validation, status, delegation, publication, and Gateway runtime cannot disagree about whether a method-only match is usable.
- If supported, method-only candidates match every service and obey the complete GRPCRoute precedence contract; if rejected, they never enter a published usable generation and have a deterministic status reason.
- Valid neighboring match shapes retain their current reachability, ordering, and delegation semantics.
- Deterministic tests cover the complete cross-layer policy without changing existing integration files.
- Workspace format/check/Clippy, documentation validation, and repository static guards pass.
