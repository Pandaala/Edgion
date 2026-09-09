# Replace Gateway Backend TLS Identities Authoritatively

## Meta

- Status: pending
- Type: correctness / sensitive-material lifecycle
- Priority: P2
- Severity: medium

## Evidence

`edgion-gateway/src/runtime/handler.rs::GatewayHandler::full_set` clears and
rebuilds the raw Gateway store but updates backend TLS identities only for keys
present in the incoming snapshot. An omitted Gateway in a smaller or empty
snapshot can therefore leave its resolved client-certificate Secret resident in
`edgion-gateway/src/backends/gateway_backend_tls_store.rs`; store counts can also
retain an identity outside the authoritative Gateway set. Explicit later removal
or replacement may repair it, but full-set processing must not rely on that event.

Revalidate these call paths before implementation. This is an authoritative
replacement problem, not proof of a concurrent-writer lost update or a network exploit.

## Scope and Acceptance

- Review a coordinated authoritative replacement API and its handler call site
  before editing. An unused store method or opportunistic lookup-time pruning
  does not establish the full-snapshot contract.
- Smaller and empty snapshots remove omitted keys and report correct counts;
  add/update/remove delta semantics remain intact.
- Preserve validity of outstanding `Arc<Secret>` readers; removing store
  ownership is not a claim that all shared or cryptographic copies are erased.
- Preserve BackendTLSPolicy TLS gating, policy identity precedence and
  BoringSSL fail-close parsing. Add no parallel store or traffic logging.
- Add local-store tests for smaller/empty snapshots, retained readers,
  incremental lifecycle and identity precedence. Keep existing integration
  files unchanged and record any required wire scenario separately.
- Run focused store/handler tests and the repository's normal verification gates.

## Related Repair Boundary

[Coalesced Gateway add replacement](replace-gateway-generations-on-coalesced-add.md)
owns delta-event generation divergence. Coordinate shared handler/store edits, but
do not claim that fixing either the delta path or full-set path fixes the other.
