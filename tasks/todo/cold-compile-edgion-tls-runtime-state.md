# Cold-Compile EdgionTls Runtime State

## Meta

- Status: pending
- Type: refactor
- Priority: P1
- Issue: N/A

## AI guide summary

Move EdgionTls serving identity, mTLS trust, and operator cipher compilation from the handshake path into immutable accepted-generation state without changing schema or TLS behavior.

## Requirements

- Compile the complete ordered serving chain and private key once per accepted generation.
- Compile the optional client CA store and normalized cipher representation outside the handshake path.
- Preserve `TlsStore` partial-update structural sharing, same-key last-known-good semantics, exact listener attachment snapshots, runtime certificate-expiry rechecks, mTLS allowlist callback ownership, and fail-close installation order.
- Keep secret-bearing compiled structures non-serializable, non-debuggable, and absent from logs and admin output.
- Preserve bounded input dimensions, BoringSSL-only Gateway support, and all B0075 full-chain and credential-cleanup regressions.

## Out of scope

- Resource/schema or user-facing configuration changes.
- Rustls Gateway implementation.
- Changes to Gateway Listener TLS compiled-identity semantics.
