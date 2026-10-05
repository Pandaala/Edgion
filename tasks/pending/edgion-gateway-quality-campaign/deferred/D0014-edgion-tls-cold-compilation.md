# D0014 — Cold-Compile EdgionTls Runtime State

- Status: resolved
- Resolution: `skills/04-review/tls/regression-guards/edgion-tls-serving-chain-install.md`
- Discovered in: B0075

## Evidence

At the time this was deferred, EdgionTls cloned Secret bytes into strings and reparsed the serving chain, private key, optional mTLS CA store, and operator cipher representation on every handshake. B0075 corrected full-chain installation and fail-close cleanup without broadening into publication ownership.

## Resolution

EdgionTls now compiles its serving chain, private key, optional client CA store,
and operator cipher string once per accepted `TlsStore` generation
(`edgion-gateway/src/tls/store/compiled_tls.rs`), and the handshake callback is a
pure consumer sharing the Gateway Listener path's atomic
`install_compiled_serving_identity`. Compile failures are carried as `Result` and
fail the handshake closed while keeping the resource in the matcher and in SNI
collision precedence. The retired shared-installer deferral is at
`skills/04-review/tls/superseded/cert-key-install-helper-deferred.md`.

## Boundary

Compile immutable secret-bearing runtime state once per accepted `TlsStore` generation while preserving partial-update structural sharing, same-key last-known-good behavior, exact attachment snapshots, runtime expiry auditing, mTLS allowlist ownership, bounded dimensions, and zero secret exposure through serialization, debug output, logs, or admin APIs.
