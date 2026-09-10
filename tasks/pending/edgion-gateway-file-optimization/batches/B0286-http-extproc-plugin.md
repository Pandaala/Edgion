# B0286 — HTTP ExtProc Plugin

- Pipeline state: changed; independent plan/final LGTM and all checks passed, local commit gate.
- Active anchor: `edgion-gateway`; refreshed baseline `d26bcd5a`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`.
- Analyst: `b0286_extproc_plugin_analysis`; full plugin-owned lifecycle/value and coverage analysis assigned. Prior B0282 supporting edits did not establish full coverage.
- B0285 committed; B0286 owns the sole serial lane. Analyst refreshed all consumers at d26bcd5a; B0285 changed only the test URI accessor in this target.
- No edits authorized; existing integrations and user handoff/WAF paths protected. Existing D0148-D0151 and queued query/opaque-header findings must not be duplicated.
- Full plugin and consumers reviewed; ImmediateResponse protobuf bytes pass through lossy String conversion, corrupting HTTP binary replies and copying valid text. Header and streamed request paths share the issue; a header-only partial fix is insufficient.
- Proposed support scope: plugin.rs, streamed.rs, runtime/traits/request_body_stream.rs, and only write_terminate_reply plus directly related tests in pg_request_body_filter_action.rs; no WAF branch edits. Correct unification section 13's stale String exception and extend existing local-response-body-is-clone-cheap-bytes knowledge.
- Plan must explicitly settle canonical gRPC valid/invalid text and exact status behavior while preserving HTTP Bytes end to end, existing caps/suppression/failure contracts. Deterministic helper/mock/ScriptedExchange/writer tests only; no socket/server or integration edits.
- At 989d2a27 target blob `9390e125a91282119939fba39bd8caca59222555`; supporting streamed `d4a2a2bfaebf8897422c4561281d6679fe0e8c28`, stream trait `42975c30e280f82621fec5e1c9bc2ededa02c381`, body action `4898e0022ecd8148b70acff0c883f647b2c61002`. Refresh after B0285 before promotion.

## Approved scope and proposed plan

- Refreshed target blob: `2a44119c1e78a2badf2a1a90e99ad8756ed97f95`; three supporting source blobs above unchanged.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`, `edgion-gateway/src/plugins/http/ext_proc/streamed.rs`, `edgion-gateway/src/plugins/runtime/traits/request_body_stream.rs`, `edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter_action.rs` (writer and directly related helper/tests only), `skills/01-architecture/00-common/05-unification-points.md` (section 13), and `skills/04-review/cpu-memory/regression-guards/local-response-body-is-clone-cheap-bytes.md`. WAF branches excluded.
- Implementer `b0286_implementation` returned plan without mutations: move protobuf Vec into Bytes, propagate through both local-response paths, decode only canonical gRPC textual messages, and update the two existing knowledge pages.
- Canonical gRPC without exact status preserves HTTP mapping and default text for absent/invalid body. Exact status including zero is preserved; absent/invalid body yields no message. Invalid UTF-8 reuses `PluginGrpcMessageBodyNotUtf8` via existing session/adapter diagnostics. HTTP always preserves bytes even with processor gRPC status.
- Deterministic tests: allocation-preserving conversion, mocked header replies and diagnostic counts, ScriptedExchange termination, pure writer message matrix and in-memory duplex binary framing. No live server or integration modifications/additions.
- Independent plan reviewer: `b0286_plan_review`; LGTM, no MustFix. Original implementer authorized. Reviewer requires request-stage/request-body qualification for mapped-default gRPC documentation; upstream response-header replacement/abort policy remains unchanged. Preserve B0282/B0283/B0285, suppression, caps, framing, cancellation, failure policy and all graded design gates.
- D0055 was already resolved on 2026-09-05; coordinator index reconciled with its completed record, without resurrecting the removed task.

## Implementation and verification

- Implementer completed the six-file plan without material deviation. HTTP carries Bytes unchanged; exact gRPC text uses the existing invalid-UTF-8 diagnostic, and the streamed writer borrows valid text. WAF branches and response-header replacement/abort policy are unchanged.
- Deterministic tests: 21/21 passed via `cargo test -p edgion-gateway --lib` with exact test-name filters. One allocation-preserving conversion test, six immediate-reply validation/mock cases, eight ScriptedExchange termination/failure/lifecycle cases, and six writer helper/in-memory duplex cases; no network or live-server tests. Initial compilation 46.63s; remaining 20 cases completed in 0.01s. No failures.
- Fresh root gates: `cargo fmt --all`, workspace/all-targets locked check (11.11s), locked Clippy (15.89s, existing warnings, exit 0), format check, agent-doc consistency, SSA force, metrics inventory, Pingora source policy, Gateway tracing boundary, English-only, integration freeze and diff whitespace all passed. No full integration suite or `cargo test --all` claim.
- Final reviewer: `b0286_final_review`, read-only; LGTM, no MustFix. All consumers, byte/text boundaries, tests, knowledge and graded gates independently checked. Supporting source changes do not claim full coverage of those files.
- Commit: pending local commit; never push.
