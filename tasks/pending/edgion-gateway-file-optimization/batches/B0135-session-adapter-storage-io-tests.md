# B0135 — Session Adapter Storage-I/O Tests

## Scope

- Baseline: `1c4f28bb2074cdf6ef557c404630c16a4118a315`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/session_adapter/storage_io_tests.rs`
- Baseline blob: `e846c23f34e6ec4fd1099f8a145141c661ba12b3`
- Implementation scope: not approved; read-only queued candidate only
- Protected: active B0134, production session/body code, all existing `edgion-tests/**`, and unrelated user paths

## Analysis boundary

Read the complete test file, production session adapter and canonical request-body storage deadline behavior. Preserve the single request `BodyStorage`, H1 reuse and H2 stream-local failure semantics, response streaming, failure policy, and hot-path production performance. Existing integration tests remain frozen.

## Handoff

- Analysis: `b0135_storage_io_analysis` traced both deliberately blocked `SpillCreate` tests through `capture_request_body_core`, `BodyStorage`, the test hook, H1 keepalive and H2 sibling-stream behavior. Decision: change; simple and test-only.
- Value: both tests currently await only the production 100 ms storage timeout. If the deadline wiring regresses, ordinary Cargo tests have no per-test timeout and can hang until the CI job limit. Canonical sibling request-body storage tests wrap equivalent blocked operations in an independent two-second watchdog.
- Proposed scope: wrap each hook-scoped capture future with `tokio::time::timeout(Duration::from_secs(2), ...)` and an actionable expectation. Preserve the inner result and every existing release, storage-failure, reuse, connection, cache-state and sibling assertion. No helper abstraction, production change, integration edit or deferred issue is justified.
- Knowledge: request-body storage deadline and replay/framing/cap/gate review guards, H1 bounded-copy intentional design, plugin development, unit-testing, root AGENTS and graded design rules.
- Freshness: promoted at `3023832d`; owned blob remains exactly `e846c23f34e6ec4fd1099f8a145141c661ba12b3`.
- Implementation: wrapped both deliberately blocked hook-scoped capture futures in independent two-second Tokio watchdogs with actionable failures. The inner `Result`, release order, H1 reuse-disable assertion, and H2 stream/cache/sibling-connection assertions are unchanged.
- Review: `b0135_final_review` independently approved with no Must Fix or Should Fix. It confirmed the watchdogs are separate from the production 100 ms deadline and existing H2 completion timeout, retain inner error typing, use a 20× scheduling margin consistent with the canonical sibling tests, and safely release blocked closures during panic unwinding.
- Verification: focused storage-I/O tests passed 2/2. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,090-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: pending local batch commit; never push.
