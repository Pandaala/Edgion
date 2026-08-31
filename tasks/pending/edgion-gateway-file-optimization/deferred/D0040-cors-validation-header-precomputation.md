# D0040 — Unify CORS Validation and Header Precomputation

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0068, CORS audit
- Category: correctness-performance
- Canonical task: [`tasks/todo/unify-cors-validation-and-header-precomputation.md`](../../../todo/unify-cors-validation-and-header-precomputation.md)

## Evidence
`CorsConfig::validation_error` and `validate_and_init` separately encode the wildcard/credentials rules. The `allow_methods`, `allow_headers`, and `expose_headers` fields remain raw strings whose fixed forms are cloned or expanded on responses, while their HTTP token/header-value grammar and distinct `*`/`**` semantics lack one canonical parsed representation.

## Why deferred
The correction belongs to the shared resource schema and both Controller/Gateway validation paths, not the Gateway-only response-container optimization. Kubernetes CEL admission work in `tasks/pending/cel-validation-rules.md` overlaps only cross-field admission; it does not replace shared semantic validation for non-Kubernetes inputs or runtime precomputation.

## Integration-test impact
No new integration issue is required. Shared resource and Gateway unit tests can lock validation parity, token grammar, wildcard semantics, and precomputed wire values; existing frozen CORS integration scenarios already cover actual/preflight behavior and hot reload.
