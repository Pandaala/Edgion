# D0077 — Wasm URL Source Label and Error Redaction

## Meta
- Status: needs-design
- Severity: high
- Discovered in: B0115 Wasm VM manager analysis
- Category: sensitive-data-and-cardinality
- Canonical task: [`tasks/todo/wasm-source-url-label-and-error-redaction.md`](../../../todo/wasm-source-url-label-and-error-redaction.md)

## Evidence
`WasmSourceSpec::metrics_label()` embeds the complete configured URL, and Gateway fetch errors also include that URL before cold-load logging. URL userinfo or query parameters may therefore expose credentials, while arbitrary paths and queries create unbounded metric label cardinality.

## Why deferred
A correct fix needs one canonical bounded, stable, credential-free source identity shared by the resources schema helper, Gateway metrics and errors, operator documentation, and inventory. Patching only the VM manager would leave another exposure or split source identity semantics.

## Integration-test impact
Deterministic resources and Gateway unit tests can cover userinfo/query redaction, bounded labels, identity stability, and fixed fetch errors. No existing integration-test edit or new integration issue is needed.
