# Guardrail Content-Encoding failure-policy fixture

This focused suite reuses the complete `EdgionBackend/SingleProvider` AI
Gateway, provider, and credential fixtures. The shared `test_server` emits a
canonical AI SSE body with repeated `Content-Encoding` values `identity` and
`gzip`, then acts as the unused Guardrail decision service.

The suite proves both `failOpen: true` and `failOpen: false` before semantic
window processing: fail-open releases the original stream, fail-close returns
`503`, the decision service is not called, and one aggregate Guardrail summary
records `reason=response-not-applicable`. No additional process, listener,
provider snapshot, or port is required.
