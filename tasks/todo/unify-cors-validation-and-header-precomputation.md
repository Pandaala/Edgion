# Unify CORS Validation and Header Precomputation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | refactor-bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Give EdgionPlugins CORS one shared validation and compiled runtime representation for method/header fields while preserving its established wire and security contracts.

## Related work

- [`tasks/pending/cel-validation-rules.md`](../pending/cel-validation-rules.md) covers Kubernetes CEL cross-field admission. This task remains responsible for shared Rust semantic validation, non-Kubernetes inputs, and Gateway runtime precomputation.
- The Gateway API `HTTPCORSFilter` has a separate schema and conformance contract and is outside this task unless independent evidence requires an explicit coordinated design.

## Requirements

- Inventory the current `allowMethods`, `allowHeaders`, and `exposeHeaders` grammar, defaults, `*`/`**` meanings, credential restrictions, response phases, and every Controller/Gateway validation and construction path.
- Use one pure shared validator for rules currently duplicated by `PluginValidation::validation_error` and `CorsConfig::validate_and_init`; keep their observable error and fail-closed behavior aligned.
- Define and validate the supported HTTP method-token and header-name/value-list grammar before runtime publication. Preserve established behavior unless a deliberately documented validation correction requires a breaking change.
- Precompute fixed wire values once in `CorsConfig` runtime caches marked `#[serde(skip)]` and `#[schemars(skip)]`; keep `allowHeaders: "**"` request-header reflection dynamic.
- Preserve the distinct EdgionPlugins meanings of `*` and `**`, wildcard/regex restrictions with credentials, preflight-only method/header emission, exact header order, and CORS constructor `validate_and_init` enforcement.
- Do not introduce a parallel Gateway-only parser, request-path tracing, new metrics, or a compatibility shim.

## Acceptance criteria

- Both validation entry points share one semantic implementation and reject the same invalid configuration.
- Static method/header/expose values are validated and prepared before the request hot path; only documented dynamic reflection remains request-dependent.
- Serde/schemars-skipped runtime caches preserve the CRD/schema and conf_sync boundaries.
- Focused resource and Gateway tests cover valid/invalid tokens, `*`/`**`, credentials, preflight/actual output, deserialization-shaped construction, and hot reload.
- Workspace check, Clippy, CRD coverage where applicable, and repository guards pass.
