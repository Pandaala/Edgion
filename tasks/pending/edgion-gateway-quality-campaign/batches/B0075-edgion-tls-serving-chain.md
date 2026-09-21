# B0075 — EdgionTls Serving Chain

## Scope

- Baseline: `f9b21be5`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned anchor file: `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs`.
- Approved knowledge files: `skills/01-architecture/02-gateway/04-tls/01-downstream-tls.md`, `skills/04-review/tls/intentional-design/cert-key-install-helper-deferred.md`, new `skills/04-review/tls/regression-guards/edgion-tls-serving-chain-install.md`, `skills/04-review/tls/regression-guards/runtime-expiry-recheck-symmetry.md`, and `skills/04-review/tls/SKILL.md`.
- Existing `edgion-tests/**`, resource/schema validation, stores/matchers, rustls behavior, user documentation, protected paths, and all other files remain forbidden.

## Context and value gate

- Analysis verdict: `change`; complexity is `complex`, so implementation is plan-first.
- EdgionTls validation accepts and checks an ordered leaf-plus-intermediate `tls.crt`, but `apply_edgion_tls_cert` parses and installs only the first PEM certificate.
- Clients that trust only the root cannot build the advertised identity because the Gateway omits required intermediates, contradicting the existing operator contract.
- Gateway Listener TLS already retains and atomically installs the complete cold-compiled chain; that separate path must remain unchanged.

## Required invariants

- Keep FIPS cipher/curve and mTLS verification setup before any server identity binding.
- Reuse the canonical strict ordered-chain parser; parse every certificate and the private key before binding anything.
- Preserve leaf-first order and runtime leaf validity auditing; bind leaf, then each intermediate in order, and private key last.
- Any parse or install failure records only generic non-secret diagnostics and leaves no usable certificate/key pair, preserving fail-close behavior of the unit-returning callback.
- Preserve overlay priority/proof, ex-data lifecycle, audit ownership, minimum-version timing, mTLS modes/client extraction, BoringSSL-only scope, redaction, and no hot-path tracing.

## Verification plan

- Independently approve the exact implementation plan before edits.
- Add a real loopback handshake proving a client trusting only the root succeeds when EdgionTls contains leaf plus intermediate.
- Prove clear-on-failure against a previously installed, client-trusted identity rather than a fresh credential-less SSL object.
- Run callback/matcher/TLS tests, full campaign gates, and frozen-path verification.
- Record the separate EdgionTls cold-compilation opportunity as a deferred task rather than broadening this correctness batch.

## Deferred findings

- D0014: cold-compile immutable EdgionTls identity, trust, and cipher state once per accepted generation instead of reparsing/cloning it per handshake.
- D0015: update the remaining mTLS reorder knowledge page that still names the removed `apply_secret_to_ssl` helper.

## Outcome

- EdgionTls now reuses the canonical strict parser, decodes the full ordered chain and key before binding, and installs leaf, intermediates, then private key.
- The connection identity is cleared before parsing and after any install failure; diagnostics are fixed non-secret categories and runtime validity uses the shared inclusive Unix-bound classifier.
- Real loopback tests prove inherited identities cannot survive early parse failure, root-only clients receive the intermediate chain, and late key failure clears the partial identity.
- Architecture and regression knowledge now describe the three BoringSSL identity paths and the full-chain/fail-close contract.
- Independent plan review required three test-oracle refinements; the final approved plan and implementation passed final review with no code corrections.
- Source commit: `18a94a49` (`Serve complete Edgion TLS certificate chains`).

## Verification

- BoringSSL callback tests: 16 passed.
- Gateway TLS matcher tests: 26 passed.
- TLS-filtered library tests: 234 passed.
- Socket ClientHello regression: 1 passed.
- `cargo fmt --all -- --check`, workspace check and Clippy, agent-doc, SSA, metrics, Pingora source, Gateway API transform, no-tracing, cargo-deny, English-only, diff, and frozen-path gates passed.
- Clippy emitted only the known Kafka `err_expect` and frozen integration DSL `redundant_async_block` warnings; cargo-deny emitted only the accepted duplicate/advisory-ignore warnings.

## Handoff

- This was the final ordered residual anchor. The `edgion-gateway` campaign ledger has no pending or in-progress rows.
- D0014 and D0015 are ready for separate tasks and do not broaden the completed campaign.
