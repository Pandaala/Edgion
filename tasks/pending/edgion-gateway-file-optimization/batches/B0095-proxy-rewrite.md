# B0095 — ProxyRewrite

## Scope
- Baseline: `fd040357`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/proxy_rewrite/{mod.rs,plugin.rs}`
- Forbidden files: resources, Controller, shared runtime/session APIs, other HTTP plugins, routing internals, documentation, every existing `edgion-tests/**` path, and protected user paths

## Analysis boundary
- Read both owned files completely and inspect their direct callers, resource validation, PluginRuntime construction, session adapter URI ownership, URI-normalization rules, header/framing safety, tests, and relevant closed findings.
- Require concrete correctness, ownership, duplication, or bounded hot-path efficiency value. Cosmetic module, naming, formatting, or test-only churn is rejected.
- Preserve canonical `PluginSession::set_upstream_uri`, current/entry URI views, `uri_finalized`, normalized-path cache refresh, retry and mirror consistency, Host/method/header semantics, body-storage caps, PluginLog ownership, and no-tracing requirements.
- Existing integration tests remain frozen. Record a separate issue only if a genuine future integration gap cannot be proven with deterministic colocated tests.

## Handoff
- Analyst: independent read-only analysis completed against `fd040357`; both owned blobs matched the recorded baseline
- Decision: simple change approved for `plugin.rs`: replace the duplicated `BodyReadError` status mapping with canonical `body_gate::record_body_read_error_status`; `mod.rs` is examined-no-change
- Implementation: exact allow-list honored; `plugin.rs` has 4 insertions and 20 deletions, with 13 ProxyRewrite and 25 BodyGate tests passing
- Final review: approved independently with no findings; exact mapping parity and all body/URI/session invariants confirmed
- Verification: `cargo fmt --all` and check mode, locked workspace check and Clippy, Gateway binary build, agent-doc/SSA/metrics/Pingora/no-tracing/English-only guards, diff check, and frozen paths all passed; Clippy emitted only pre-existing warnings
- Coverage: `mod.rs` examined-no-change; `plugin.rs` changed
- Integration: existing integration tests remained frozen and sufficient; no follow-up integration issue was created
- Commit: `b5bdf94a`
