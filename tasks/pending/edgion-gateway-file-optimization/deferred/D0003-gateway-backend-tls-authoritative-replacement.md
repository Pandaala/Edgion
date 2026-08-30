# D0003 — Replace Gateway Backend TLS Identities Authoritatively

## Meta

- Status: needs-discussion
- Severity: medium
- Discovered in: B0039, `edgion-gateway/src/backends/gateway_backend_tls_store.rs`
- Category: correctness

## Evidence

`GatewayHandler::full_set` clears and rebuilds the authoritative Gateway store, but it updates `GatewayBackendTlsStore` only for Gateway keys present in the incoming snapshot. A Gateway omitted from a smaller or empty authoritative snapshot is therefore not removed from the backend TLS identity store. Its resolved client-certificate Secret remains resident and the admin store count remains stale until an explicit later remove or same-key update occurs.

Partial add, update, and remove events do maintain the TLS store correctly. Production mutations are serialized by the Gateway store write guard, so this is an authoritative replacement gap rather than a concurrent-writer lost update.

## Why deferred

A complete fix requires coordinated authoritative replacement across `GatewayBackendTlsStore` and `GatewayHandler::full_set`. Adding an unused target-only replacement method, lookup-time dependency, capacity bound, or pruning heuristic would not establish the ownership contract.

## Impact and open questions

- Omitted Gateway identities can retain sensitive certificate/key material indefinitely.
- Admin statistics can report identities outside the current authoritative Gateway generation.
- A captured Gateway key can retain bounded access to the stale identity during concurrent generation transition.
- Define full-snapshot replacement while preserving incremental add/update/remove behavior.
- Preserve `Arc<Secret>` reader validity, BackendTLSPolicy-required TLS gating, policy identity precedence, and BoringSSL fail-close parsing.

## Candidate next step

Design an authoritative replacement API and wire it into `GatewayHandler::full_set`. Add smaller-snapshot and empty-snapshot tests that prove omitted-key removal and correct counts, plus incremental lifecycle and identity-precedence regressions.
