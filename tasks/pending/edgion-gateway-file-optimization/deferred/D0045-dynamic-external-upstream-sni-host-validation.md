# D0045 — Reject Unsafe DynamicExternalUpstream SNI and Host Overrides

## Meta
- Status: needs-implementation
- Severity: high
- Discovered in: B0073, DynamicExternalUpstream audit
- Category: correctness-security
- Depends on: B0073
- Canonical task: [`tasks/todo/reject-unsafe-dynamic-external-upstream-sni-host-overrides.md`](../../../todo/reject-unsafe-dynamic-external-upstream-sni-host-overrides.md)

## Evidence
Shared validation does not inspect explicit `sni` or `overrideHost`. With `tls: true`, an explicit empty SNI remains `Some("")` instead of defaulting to the domain and reaches the supported Pingora BoringSSL connector path, which disables SNI and certificate verification for an empty peer SNI. An `overrideHost` value that cannot be encoded as an HTTP header value fails request-time header conversion; that error is discarded and the original Host is retained. A header-value-valid but Host-authority-invalid value is not rejected by this conversion and may be forwarded.

## Why deferred
The fix belongs in shared Controller/Gateway resource validation and schema-facing EN/ZH documentation, not the B0073 request-path ownership patch. It must reuse canonical hostname/authority and header-value rules without narrowing currently supported internal names or IP forms accidentally.

## Integration-test impact
No new integration issue is required. Shared resource validation tests can prove rejection before publication, and both Controller and Gateway already consume the same validation path.
