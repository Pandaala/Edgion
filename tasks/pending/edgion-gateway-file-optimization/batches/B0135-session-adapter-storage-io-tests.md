# B0135 — Session Adapter Storage-I/O Tests

## Scope

- Baseline: `1c4f28bb2074cdf6ef557c404630c16a4118a315`
- Pipeline state: analysis-ready
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
- Review: pending promotion after the active serial batch.
- Verification: pending.
- Commit: pending local commit after its own serial gate; never push.
