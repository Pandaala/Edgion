# ARCH-002 implementation plan V1

## Approved outcome and feasibility

User approved candidate V1 on 2026-10-11. One canonical request-body owner will define and export exactly `BodyReadError`, `BodyReadResult`, `RequestBodyChunkConsumer`, and `RequestBodySnapshotReader`. Move declarations including doc comments, Display/Error implementations, `async_trait`, Send bounds, method signatures and default bodies byte-for-byte from session.rs to request_body/contracts.rs. Source actually contains **eight**, not nine, BodyReadError variants; all eight are preserved without addition.

The private `contracts` module imports only `bytes::Bytes` and `async_trait::async_trait`; request_body/mod.rs publicly exports the four types. PluginSession privately imports body-owned contracts along with its existing BodyOperationError/BodyScanResult. No compatibility export remains in session. Body storage/state/scanners/operations no longer import fundamental contracts from plugin runtime. `RequestBodyInspectionCapture` stays in session, and observer PluginLog stays unchanged: complete plugin independence is not claimed.

Feasibility follows unchanged object-safe trait methods: RequestBodySnapshotReader retains its async_trait-erased Send future and Send trait-object bound; owned reader return values and consumer lifetime arguments remain identical. Module relocation does not alter default behavior, concrete implementer bodies, mockall signatures, ABI-relevant shapes or borrowing. No body API invokes a name by string. Existing glob/super::* test imports continue through their owning production module's private imports; tests requiring explicit old paths receive import changes only. A future snapshot capability changes its definition in request_body beside storage implementation, then updates feature consumers; consumer fanout is not claimed to disappear.

## Frozen edit allow-list

Existing production and unit files below may change only contract paths/imports, the declaration removal, and the mod/export insertion. No executable consumer/implementation body edits. Mixed session imports are split, retaining session-owned symbols there. No tests are relocated, deleted, added, renamed, ignored or reconfigured; visibility and source-relative paths stay unchanged.

- `edgion-gateway/src/ai/guards/request/plugin.rs`
- `edgion-gateway/src/ai/http/proxy.rs`
- `edgion-gateway/src/ctx.rs`
- `edgion-gateway/src/plugins/http/dsl/lang/body_scanners.rs`
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`
- `edgion-gateway/src/plugins/http/form_json_transform/plugin.rs`
- `edgion-gateway/src/plugins/http/grpc_json_transcoder/plugin.rs`
- `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs`
- `edgion-gateway/src/plugins/http/proxy_rewrite/plugin.rs`
- `edgion-gateway/src/plugins/http/request_mirror/plugin.rs`
- `edgion-gateway/src/plugins/http/waf/json_capture.rs`
- `edgion-gateway/src/plugins/http/waf/mod.rs`
- `edgion-gateway/src/plugins/http/wasm/plugin.rs`
- `edgion-gateway/src/plugins/runtime/body_gate.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `edgion-gateway/src/plugins/runtime/traits/session.rs`
- `edgion-gateway/src/plugins/runtime/traits/waf_session.rs`
- `edgion-gateway/src/request_body/body_scanners.rs`
- `edgion-gateway/src/request_body/mod.rs`
- `edgion-gateway/src/request_body/operation.rs`
- `edgion-gateway/src/request_body/state.rs`
- `edgion-gateway/src/request_body/state/enabled.rs`
- `edgion-gateway/src/request_body/storage.rs`
- `edgion-gateway/tests/unit/ctx_test/request_body_cap_tests.rs`
- `edgion-gateway/tests/unit/plugins/http/dsl/lang/builtins_test/body_tests.rs`
- `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/body_value.rs`
- `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/error_policy.rs`
- `edgion-gateway/tests/unit/plugins/http/dsl/mod_test/http_call_tests.rs`
- `edgion-gateway/tests/unit/plugins/http/ext_proc/plugin_test.rs`
- `edgion-gateway/tests/unit/plugins/http/graphql_rate_limit/plugin_test.rs`
- `edgion-gateway/tests/unit/plugins/http/json_schema_validation/plugin_test.rs`
- `edgion-gateway/tests/unit/plugins/http/request_access_policy/plugin_test.rs`
- `edgion-gateway/tests/unit/plugins/http/waf/mod_test.rs`
- `edgion-gateway/tests/unit/plugins/http/wasm/plugin_test.rs`
- `edgion-gateway/tests/unit/plugins/runtime/body_gate_test.rs`
- `edgion-gateway/tests/unit/plugins/runtime/conditions/evaluator_test.rs`
- `edgion-gateway/tests/unit/plugins/runtime/plugin_runtime_test.rs`

Existing canonical documentation/knowledge allow-list:

- `skills/01-architecture/02-gateway/05-plugin-system.md`
- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`
- `skills/04-review/http-body/SKILL.md`

Allowed creations:

- `edgion-gateway/src/request_body/contracts.rs`
- `skills/04-review/http-body/intentional-design/request-body-read-contract-ownership.md`

Allowed deletions: none. No manifests/dependencies, other crates, benchmarks, edgion-tests, source-review artifacts, ARCH-001/029, WAF logic, stage policy, transport or observer edits. Task records under this task directory may be created/updated; the coordinator owns reconciliation of its main task. Substantive documentation: describe the canonical owner in plugin-system architecture and extension guide, and record ARCH-002 as bounded intentional-design with benefit, alternatives, costs, evidence and re-evaluation triggers. No operator behavior changes require user-facing documentation updates.

## Sequence and rollback

1. Independent plan review binds V1 plan and 01-baseline.json. Revalidate hashes before editing.
2. Move exact declaration span into contracts.rs; add private module/public exports; privately import in session.rs. Migrate frozen actual consumer imports/qualified paths only.
3. Update the two canonical guides, intentional-design page and owning index. Preserve guidance on capture/admission/response streaming.
4. Apply cargo fmt, verify declaration equality (excluding import/module packaging), unique definitions and absence of old paths, and compare source test identities (function names, attributes and paths) against baseline.
5. Run required checks and existing regressions below. Record results/limits; obtain fresh independent result review. Corrections stay within allow-list and approved unchanged-behavior contract; material deviations require renewed design review.

Rollback restores only task-owned modified files from frozen baseline and removes only the two task-owned creations after checking they have no external changes. Never reset/stash unrelated changes. Work exclusively in detached isolated worktree /Volumes/ExtStore/ws6/Edgion-arch002 at ef69a780, with independent target cache supplied by coordinator; no commit/push.

## Preserved runtime gates

Immutable request-only BodyStorage generations, independent replay cursors, source/backing lifetime across mutation/await, RequestMirror isolation reservation and returned-chunk lifetime, actual EOF and byte-count parity, physical capture cap versus logical read admission, proven empty versus uncaptured, prefix sealing versus EOS, stage gates, retries/cancellation, storage I/O deadlines and failure priorities remain identical. No new allocations/copies/locks/cache/dispatch/logging/metrics/error text. English-only content, workspace DAG, binary-owned logging, canonical access-log ownership, sensitive redaction, observability restraint, cross-cutting unification, unit-test layout and identity rules are hard gates.

## Verification

Structural checks: byte-for-byte moved declaration span, exactly one definition each in contracts.rs, no public legacy exports or session-qualified contract references, no fundamental plugin-runtime imports in body storage/state/operation/scanners, diff limited to approved imports/packaging and substantive docs. Test identity/attribute/path comparison proves no protected test changes; fresh filtered runs verify actual module and mock wiring.

Run in isolated worktree after coordinator cache preparation:

- `cargo fmt --all`, then `cargo fmt --all -- --check` and `git diff --check`.
- `cargo check --workspace --all-targets` and `cargo clippy --workspace --all-targets` (default supported BoringSSL/allocator features; no all-features).
- `cargo test -p edgion-gateway --lib request_body::` (storage/state/operation/scanner snapshot generations, admission, replay, isolation, deadlines).
- `cargo test -p edgion-gateway --lib plugins::runtime::session_adapter::tests::` (real adapter EOF/parity, stage/bodyless/physical/native admission and storage IO).
- `cargo test -p edgion-gateway --lib plugins::runtime::body_gate::tests::` (error policy/physical-logical cap semantics).
- `cargo test -p edgion-gateway --lib plugins::http::waf::` (fused borrowed consumers and bounded prefix/full behavior).
- `cargo test -p edgion-gateway --lib plugins::http::request_mirror::` (detached readers/isolation/lifetime).
- `cargo test -p edgion-gateway --lib plugins::http::dsl::lang::builtins::tests::` (opaque reader cancellation/error wiring), and `cargo test -p edgion-gateway --lib ctx::tests::request_body_cap_tests::` (context cap path).
- Python guards: `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`, `check_pingora_source_policy.py`, `generate_gateway_api_transform_contract.py --check`, `check_unit_test_layout.py`, `check_gateway_no_tracing.py` from cicd/checks/.

If a guessed Rust filter matches zero tests, inspect fresh `--list` output and correct the filter to the existing identity, not the test. Full `cargo test --all` is not authorized. No dependency/resource edits make cargo audit/CRD coverage applicable; ordinary guard failures are reported and traced against baseline rather than silently suppressed. Historical runs are not fresh results. Any pre-existing failure remains an explicit unresolved gate unless project/user authority grants an applicable exception.

## Evidence binding

01-baseline.json hashes every frozen existing edit file plus AGENTS.md and graded design rules. Only repository-root AGENTS.md exists in this worktree. Original knowledge loaded progressively: skills/SKILL.md; 00-design-rules; architecture plugin system/extension guide; 04-review root, routing and http-body index; request-body storage I/O deadline and admission snapshot pages; 05-testing index and unit-testing guide; 07-tasks index. Source inspected original declarations, body exports, actual consumers, snapshot admission and RequestMirror isolation. Prior value record's absent historical WAF spec is not a design input. No build/test/prototype has run during plan creation.
