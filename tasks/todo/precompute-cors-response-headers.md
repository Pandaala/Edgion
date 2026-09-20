# Precompute CORS Response Headers

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-20 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Precompute the static EdgionPlugins CORS response-header values before runtime publication while preserving the established wire format, wildcard behavior, and dynamic request-header reflection.

## Requirements

- Inventory the current `allowMethods`, `allowHeaders`, and `exposeHeaders` response phases and the distinct meanings of `*` and `**`.
- Precompute fixed wire values once in `CorsConfig` runtime caches marked with both `#[serde(skip)]` and `#[schemars(skip)]`.
- Keep `allowHeaders: "**"` request-header reflection dynamic.
- Preserve exact header order, case, credential restrictions, preflight-only method/header emission, and the existing fail-closed construction behavior.
- Keep validation and initialization aligned for Controller ingest, Gateway construction, and hot reload.
- Do not add request-path tracing, a parallel Gateway-only parser, schema-visible cache fields, or compatibility shims.

## Scope

### In scope

- EdgionPlugins CORS runtime header preparation and its focused resource, Gateway, and hot-reload tests.

### Out of scope

- Native Gateway API `HTTPCORSFilter` behavior and schema.
- Changes to the accepted HTTP token grammar completed by the preceding validation fix.

## Acceptance criteria

- Static method, header, and expose-header values are prepared before the request hot path.
- Dynamic reflection remains request-dependent only where the documented `**` behavior requires it.
- Serde- and schemars-skipped caches preserve the CRD and conf_sync boundaries.
- Focused tests cover `*` and `**`, credentials, preflight and actual responses, deserialization-shaped construction, and hot reload.
- Workspace check, Clippy, relevant documentation guards, and targeted tests pass.

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-resources` | Own the validated and precomputed CORS runtime representation. |
| `edgion-gateway` | Consume prepared values without rebuilding static header strings on requests. |
| `edgion-tests` | Verify runtime behavior and hot reload. |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-20 | Split precomputation from the completed token-validation work. | The validation defect is fixed, while request-path header preparation remains independent follow-up work. |
