# Complete Native Gateway API CORS Validation Parity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Make native Gateway API `HTTPCORSFilter` validation and defaults match the vendored v1.5 contract on Kubernetes and non-Kubernetes ingest paths.

## Requirements

- Inventory every relevant v1.5 CORS constraint: origin scheme/host/port and length, list bounds and set semantics, wildcard exclusivity for allowed origins, methods, and request headers, allowed methods, header-name grammar, and `maxAge` minimum/default. Preserve the resolved [credentialed exposure policy](../../skills/04-review/cors/intentional-design/native-credentialed-exposure-policy.md): credentials-enabled native CORS rejects any exact `*` exposure entry.
- Enforce the same semantic validator through Controller invalid-rule/status handling and Gateway construction without adding a watcher-local OpenAPI engine.
- Make omitted `maxAge` behavior match the v1.5 default while preserving valid explicit values.
- Keep this native Gateway API contract separate from the
  [EdgionPlugins CORS validator and precomputation](unify-cors-validation-and-header-precomputation.md).
  Share canonical primitives where their semantics agree, without merging the two admission contracts.
- Preserve immutable publication, established valid wire behavior, restrained diagnostics, and English-only content.
- Do not modify existing integration-test files.

## Acceptance criteria

- Non-Kubernetes inputs cannot publish a native CORS value that the relevant v1.5 admission constraints would reject.
- Controller and Gateway reach the same validation decision through the canonical validator; Controller status carries the operator-facing invalid-field details while Gateway construction rejects the invalid runtime.
- Valid and invalid boundary tests cover every field class, allowed-origin/method/request-header wildcard rule, list limit, port boundary, and omitted/explicit `maxAge` behavior. Credentialed expose-header wildcard semantics remain excluded under the linked task.
- A standalone non-Kubernetes integration scenario is added only if unit and status tests cannot prove the ingest boundary.
