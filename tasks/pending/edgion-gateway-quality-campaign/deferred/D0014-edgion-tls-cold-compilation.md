# D0014 — Cold-Compile EdgionTls Runtime State

- Status: ready-for-separate-task
- Canonical task: `tasks/todo/cold-compile-edgion-tls-runtime-state.md`
- Discovered in: B0075

## Evidence

EdgionTls currently clones Secret bytes into strings and reparses the serving chain, private key, optional mTLS CA store, and operator cipher representation on every handshake. B0075 corrected full-chain installation and fail-close cleanup without broadening into publication ownership.

## Boundary

Compile immutable secret-bearing runtime state once per accepted `TlsStore` generation while preserving partial-update structural sharing, same-key last-known-good behavior, exact attachment snapshots, runtime expiry auditing, mTLS allowlist ownership, bounded dimensions, and zero secret exposure through serialization, debug output, logs, or admin APIs.
