# B0134 — Plugin Session Adapter

## Scope

- Baseline: `b6060c7acf8796cedf59d1d1adc692e0f98accdc`; revalidated at `13e0890f`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- Baseline blob: `8b6a310abdc698e74a96907d5f0e380abba9107e`
- Implementation scope: not approved; read-only candidate only
- Protected: active B0131 and campaign records, all existing integration tests, and unrelated user paths

## Analysis boundary

Read the complete Pingora-backed `PluginSession` adapter and trace every trait method to request context/session ownership, URI/header/body/TLS/client-IP semantics, plugin consumers, unit/integration coverage, and review precedent. Inspect `session_adapter/storage_io_tests.rs` as direct coverage without claiming its ledger row unless the analysis proves the files must be handled together.

Preserve canonical URI normalization/finalization, direct-peer versus remote-IP identity, downstream TLS facts, request-body single-cache policy, bounded plugin outbound exchanges, failure/access-log ownership, binary logging, sensitive-data safety, response streaming/backpressure, hot-path allocation discipline, and cross-cutting unification.

## Handoff

- Analysis: `b0134_session_analysis` read the full adapter, storage-I/O tests, every trait method family, consumers, request context/session ownership, URI/header/body/TLS/IP behavior and review precedent. Decision: change; complex/plan-first because the narrow edit affects HTTP parsing, the request hot path and authentication consumers.
- Value: `cookie_value` uses `HeaderMap::get(COOKIE)` and misses valid later Cookie fields, including HTTP/2-split cookies. This can make JWT/KeyAuth, CSRF, OIDC, KeyGet, DSL/templates, routing/outbound values and access-log projection treat a present cookie as absent. Iterate `get_all(COOKIE)` in field order, preserve per-field parsing/case-sensitive first match, avoid intermediate allocation and fail conservatively on malformed UTF-8.
- Proposed scope: this file only with colocated tests for a later-field match and deterministic first-match ordering. `storage_io_tests.rs` remains pending and unchanged. No integration edit is needed.
- Deferred boundaries: D0046 owns invalid final local-reply status. `session_persistence.rs` and `backends/selector.rs` have similar first-Cookie reads but remain subject to their own pending file audits.
- Knowledge: root/design rules, plugin architecture, URI normalization, Rust/log/access-log/testing guidance, and focused body/IP/TLS/credential-stripping/OIDC/H2 review knowledge.
- Review: pending promotion after the active serial batch.
- Verification: pending.
- Commit: pending local commit after its own serial gate; never push.
