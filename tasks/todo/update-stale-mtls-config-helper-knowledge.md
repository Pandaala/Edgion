# Update Stale mTLS Config Helper Knowledge

## Meta

- Status: pending
- Type: docs
- Priority: P2
- Issue: N/A

## AI guide summary

Revalidate and update `skills/04-review/tls/regression-guards/mtls-config-reorder.md`, which still names the removed `apply_secret_to_ssl` helper after TLS identity installation was split into current Gateway and EdgionTls paths.

## Requirements

- Trace current BoringSSL mTLS configuration and server-identity installation symbols before editing.
- Replace only stale helper and ownership statements; do not rewrite valid historical evidence.
- Preserve the rule that FIPS policy and mTLS verification configuration are applied before any server identity binding.
- Keep Gateway compiled identity, EdgionTls runtime identity, fail-close, logging, redaction, and rustls-boundary descriptions accurate.
