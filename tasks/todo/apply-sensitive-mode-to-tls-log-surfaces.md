# Apply Sensitive Mode to TLS Log Surfaces

## Meta

- Status: pending
- Type: bug
- Priority: P2
- Severity: medium
- Source: Gateway campaign B0206 / D0116

## Evidence and Acceptance Criteria

`routes/tls/proxy.rs` TlsContext SNI/error fields lack sensitive serializers;
both `proxy.rs` and `passthrough_proxy.rs` per-listener JSON directly insert
those values. `observe/logs/tls_log.rs::log_tls` serializes without additional
masking. Logging policy requires SNI to respect sensitive mode. Dynamic error
paths also require review; the observed invalid-IP error normally contains
"unknown", so do not claim every normal client IP leaks through that branch.

- Coordinate context serialization and both listener JSON surfaces under one
  exact reviewed allow-list. Verify Plain/Replace/Wrap and None shape.
- Preserve runtime SNI, routing and PROXY protocol authority; mask only output.
- Use isolated subprocess tests for process-global sensitive mode.
- Reuse canonical masking; add no parallel tracing or duplicate traffic logs.
- Keep integration files frozen in this campaign; defer any new wire coverage.
- B0206 ownership optimization does not fix this privacy gap.
