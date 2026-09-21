# B0073 — Gateway TLS Entry Identity

## Scope

- Baseline: `826ae53c`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/runtime/matching/tls.rs`.
- Approved support files: `edgion-gateway/src/runtime/matching/tests/tls_test.rs` and `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`.
- Existing `edgion-tests/**`, rustls behavior, protected user paths, and all other files remain forbidden.

## Context and value gate

- Analysis verdict: `change`; complexity is `complex`, so implementation is plan-first.
- `GatewayTlsEntry` duplicates Gateway/listener identity already owned by its `ResolvedListenerAttachment`, including independently constructible proof fields and redundant owned strings.
- Canonicalizing identity on the attachment removes retained allocations and representable authorization/logging drift without changing matching, certificate compilation, frontend mTLS, publication, or output.

## Required invariants

- Preserve exact, wildcard, and catch-all lookup plus RFC 6125 single-label wildcard behavior.
- Preserve collision order by creation time, namespace, Gateway name, and listener name; warning content remains bounded and equivalent.
- The exact resolved attachment and proof remain the sole EdgionTls overlay authorization key; stale or unresolved proofs fail closed.
- Preserve complete serving-certificate/frontend-CA compilation, StrictReject ownership, atomic publication, old-generation Arc lifetime, callback order, and canonical SSL/access-log ownership.
- No rustls implementation, new logging/metrics/tracing, secret material, compatibility aliases, schema/protocol change, or unrelated cleanup.

## Verification plan

- Independently approve the exact implementation plan before edits.
- Update matcher construction/order/warnings, callback metadata, fixtures, and assertions through the canonical attachment.
- Prove removed symbols have no remaining consumers and all constructors use one attachment identity.
- Run matcher and callback tests, full campaign gates, and frozen-path verification.

## Deferred findings

- None.

## Outcome

- Removed six redundant Gateway/listener identity fields and made `ResolvedListenerAttachment` the sole retained identity and proof on each `GatewayTlsEntry`.
- Matcher construction now computes the Gateway proof once and builds one entry before exact/wildcard versus catch-all insertion; collision order, warnings, certificate/frontend-mTLS compilation, and publication remain unchanged.
- BoringSSL matched metadata now reads the same canonical attachment already used for exact EdgionTls overlay authorization.
- Independent plan and final reviews approved the change with no deviations, corrections, or deferred findings.
- Source commit: `1d16a7ee` (`Canonicalize gateway TLS entry identity`).

## Verification

- Matcher tests: 26 passed.
- BoringSSL callback tests: 14 passed.
- TLS-filtered library tests: 311 passed in implementation verification; independent focused TLS review also passed.
- Removed-field and constructor searches confirmed one attachment per remaining constructor and no duplicate-field consumers.
- `cargo fmt --all -- --check`, workspace check and Clippy, agent-doc, SSA, metrics, Pingora source, Gateway API transform, no-tracing, cargo-deny, English-only, diff, and frozen-path gates passed.
- Clippy emitted only the known Kafka `err_expect` and frozen integration DSL `redundant_async_block` warnings; cargo-deny emitted only the accepted duplicate/advisory-ignore warnings.

## Handoff

- Next ordered anchor: `edgion-gateway/src/runtime/server/error_response.rs` (B0074), after current-HEAD revalidation and mandatory plan review for its queued terminal-header proposal.
