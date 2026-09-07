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

- Inventory every relevant v1.5 CORS constraint: origin scheme/host/port and length, list bounds and set semantics, wildcard exclusivity for allowed origins, methods, and request headers, allowed methods, header-name grammar, and `maxAge` minimum/default. Credentialed `exposeHeaders: ["*"]` behavior remains owned by [`decide-credentialed-native-cors-expose-headers-wildcard.md`](decide-credentialed-native-cors-expose-headers-wildcard.md).
- Enforce the same semantic validator through Controller invalid-rule/status handling and Gateway construction without adding a watcher-local OpenAPI engine.
- Make omitted `maxAge` behavior match the v1.5 default while preserving valid explicit values.
- Keep this native Gateway API contract separate from D0040's EdgionPlugins CORS validator and representation.
- Preserve immutable publication, established valid wire behavior, restrained diagnostics, and English-only content.
- Do not modify existing integration-test files.

## Acceptance criteria

- Non-Kubernetes inputs cannot publish a native CORS value that the relevant v1.5 admission constraints would reject.
- Controller and Gateway reach the same validation decision through the canonical validator; Controller status carries the operator-facing invalid-field details while Gateway construction rejects the invalid runtime.
- Valid and invalid boundary tests cover every field class, allowed-origin/method/request-header wildcard rule, list limit, port boundary, and omitted/explicit `maxAge` behavior. Credentialed expose-header wildcard semantics remain excluded under the linked task.
- A standalone non-Kubernetes integration scenario is added only if unit and status tests cannot prove the ingest boundary.
