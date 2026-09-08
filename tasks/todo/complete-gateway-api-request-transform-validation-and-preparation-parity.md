# Complete Gateway API Request-Transform Validation and Preparation Parity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Validate and prepare Gateway API RequestRedirect and URLRewrite filters consistently across every ingest path so malformed configuration is rejected before traffic and runtime state is immutable and typed.

## Requirements

- Define canonical validation for filter type/config pairing, path-modifier shape, scheme, hostname, port, redirect status, list/cardinality bounds, and incompatible filter combinations.
- Apply it to HTTPRoute rule/backend lists, delegated or effective rules, and EdgionPlugins preserve-unknown-fields input across Kubernetes, filesystem, and etcd ingestion.
- Keep Controller invalid-rule/status results and Gateway defensive construction consistent while valid sibling rules remain active.
- Prepare typed immutable transform state once and preserve path-element, query, host/port, explicit-scheme, local-response, and streaming semantics.
- Use static non-sensitive errors and canonical access-log diagnostics; add no tracing or parallel path-normalization mechanism.
- Do not modify existing integration-test files.

## Acceptance criteria

- Shared-resource tests cover every structural and value boundary.
- Controller tests cover rule/backend invalidation and valid sibling retention for all ingestion-equivalent shapes.
- Gateway tests prove malformed input fails closed before upstream and valid plans execute without request-time parsing.
- A standalone integration issue is added only if unit tests cannot prove the ingest boundary.
