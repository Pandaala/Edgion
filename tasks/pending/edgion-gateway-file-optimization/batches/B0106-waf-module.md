# B0106 — WAF HTTP Plugin Module

## Scope
- Baseline: `512d485d`
- Pipeline state: ready-to-commit
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/http/waf/mod.rs`
- Baseline blob: `f0ac368e003937bd5e404ceffda030a567d24d23`
- Support blobs: resources WAF config = `38590337a5e46279bbf9099eecb4e31235093b18`; Gateway plugin store = `76a0bfc8fcf78d4be16490cb743b1a5ade338c58`
- Implementation allow-list: `edgion-gateway/src/plugins/http/waf/mod.rs`; `edgion-resources/src/resources/edgion_plugins/plugin_configs/waf.rs`; `edgion-gateway/src/plugins/http/plugin_store.rs`; test-only support `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs`
- Forbidden files: session traits/adapters, runtime/context, `edgion-waf`, Controller, docs, metrics/access-log schemas, Cargo, every existing `edgion-tests/**` path, campaign-protected user paths, and every file outside the exact allow-list

## Analysis boundary
- Read the complete module and trace its configuration, compilation, request/response phase, body-storage, logging/access-log, native WAF engine, plugin runtime, and test boundaries before deciding value.
- Preserve canonical request-body ownership, streaming upstream responses, binary-owned operational logging, request-level access-log ownership, sensitive-data redaction, observability restraint, and cross-cutting unification.
- Existing integration tests remain a frozen validation floor. Prefer deterministic crate-local coverage; any genuinely necessary new integration coverage becomes a separate issue.

## Handoff
- Analyst: independent read-only analysis completed at `512d485d`; the target blob remained `f0ac368e003937bd5e404ceffda030a567d24d23`
- Decision: change. Centralize trimmed-empty rule rejection in `WafConfig::compile_generation`, preserve Controller/Gateway error parity and last-known-good semantics, use saturating monotonic elapsed time, and remove the temporary method/remote-IP vectors before transaction-owned copying.
- Complexity: complex/plan-first because the batch spans the shared security-validation boundary, Gateway publication parity tests, and a request hot path across three exact files.
- Preserved behavior: one strict compile entry, fixed non-sensitive `rules must not be empty` error, current compile-error metric and invalid-update retention, winner/mode/priority and Phase 1–5 ordering, canonical BodyStorage/raw-stream selection and physical cap, generic fail-close response, bounded WAF access log, and no tracing.
- Tests: use a valid no-op rule in the generic log-suppression test; add blank-rule constructor and Controller/Gateway error-set parity regressions; update the method-call expectation after borrowing request-head bytes.
- Plan review: independently approved with no Must Fix. Exact shared-error assertions, first-error behavior, synchronous transaction copying, no-op fixture semantics, canonical saturating elapsed time, three-file scope, and frozen integration coverage were confirmed.
- Deferred: D0071 owns any redesign to eliminate per-header double materialization across PluginSession and `edgion-waf` transaction ingestion.
- Integration: the frozen WafDynamic scenario already covers phases, generation switching, invalid-update retention, recovery, deletion, and rebuild. Blank-source parity is deterministic crate-local coverage; no integration issue is needed.
- Implementation: only the three-file allow-list changed. Empty-rule rejection now lives in the shared compiler entry, validation delegates to it, elapsed time is saturating, and request method/remote-IP bytes are borrowed until synchronous transaction copying. Focused exact-error, parity, constructor, fixture, and mock-call tests were updated as approved.
- Implementer verification: resources WAF 6/6, Gateway WAF 10/10, and Controller/Gateway parity passed; locked targeted Gateway/resources checks, format, diff, no-tracing, English-only, and frozen-path guards passed.
- Mechanical containment: HEAD and all baseline blobs remain unchanged; the code diff is exactly the frozen three-file allow-list, and existing integration files remain untouched.
- Final review: Issues Found. Production changes and value case were approved, but `gateway_api_filters/extension_ref.rs` retains a directly caused two-call method mock expectation while the optimized WAF path calls it once. That exact ExtensionRef unit test fails.
- Must Fix: update only the stale expectation after an implementation-plan scope revision and independent reapproval of the exact support-test path.
- Correction plan review: approved with no findings. The test-only support blob is `475dcca4a63fb1d44f7ba598babd9698f47dded8`; one `.times(2)` to `.times(1)` change is necessary and sufficient while preserving ExtensionRef WAF execution, log suppression, outer attribution, and frozen integration coverage.
- Correction implementation: changed only the approved ExtensionRef mock expectation. The named regression passed 1/1, WAF suite 10/10, parity test 1/1, and format/diff/no-tracing/English/frozen guards passed; code containment is exactly the four-file allow-list.
- Final re-review: LGTM with no Must Fix or Should Fix. The reviewer confirmed the prior failure is resolved, all production value/invariants remain valid, D0071 stays deferred, and no integration issue is needed.
- Verification: resources WAF 6/6, Gateway WAF 10/10, Controller/Gateway parity 1/1, and the ExtensionRef regression 1/1 passed. `cargo fmt --all` and `--check`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen-path guards passed on the current HEAD. Clippy retained only the existing warning set.
- Commit: pending
