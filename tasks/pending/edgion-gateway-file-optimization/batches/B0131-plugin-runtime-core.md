# B0131 — Plugin Runtime Core

## Scope

- Baseline: `b71f37f2fe8b3ae837bcfb08e159d540a0b97715`; promoted at `b6060c7a`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`
- Baseline blob: `f59d02e39aa98981486850488b8de77ef3c29d94`
- Candidate implementation allow-list: owned file only, including its existing unit-test module
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete PluginRuntime construction, validation, stage dispatch, immutable publication, all direct plugin/runtime consumers, tests, and relevant review knowledge. Revisit B0121 support without assuming it completes this file's audit.
- Preserve plugin order, condition/rejection policy, request-body and response-streaming ownership, failure policy, canonical access-log diagnostics, binary-owned logging, redaction, observability restraint, and hot-reload generation behavior.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: `b0131_plugin_runtime_analysis` read all 3,643 lines and traced constructors, four stage dispatchers, condition/body/dye wrapping, immutable publication, HTTP/gRPC consumers, and existing tests/review precedent. `b0129_freshness` independently revalidated the entire file at `b6060c7a`; its blob is unchanged. Decision: change; complex because three live request/response dispatch loops are touched.
- Value: replace three `Instant::elapsed().as_micros() as u64` expressions with one private checked, saturating microsecond helper, following the WAF precedent and canonical time rules. Rust 1.96.1 currently saturates reversed `elapsed()` samples, so the demonstrated benefit is explicit contract alignment plus prevention of `u128 → u64` wraparound, not a current panic claim. Preserve normal timing values, log omission, plugin order, rejection behavior, and streaming; add a deterministic future-Instant regression. No new allocation or observability.
- Deferred: the existing route/backend response-body-dispatch task and D0081/D0082/D0089 retain their scopes; no new issue or integration test is required.
- Later queued evidence: B0132 raised D0095, a not-yet-reproduced policy/invalid-generation retention interaction. Keep that publication investigation separate from this proposed timing-only change and revalidate the boundary on promotion.
- Freshness boundary: D0095 lives in generation compilation/publication and does not read timing or `PluginLog::time_cost`; B0131 must not touch that path.
- Plan: (1) add the private checked/saturating microsecond helper; (2) replace only three timing assignments; (3) add a future-Instant zero regression; (4) run the focused and complete PluginRuntime module tests plus full gates. Preserve all timer boundaries, log omissions, stage behavior, streaming and D0095 ownership.
- Plan review: `b0131_plan_review` APPROVED at `13e0890f`, no Must Fix or Should Fix.
- External-gate diagnostic: `b0131_gate_diagnostic` attributed six changed and six newly discovered integration files to committed external updates `6ab45e17`/`8cbf7773`/`13e0890f`, not this batch. The frozen manifest was regenerated at external HEAD `69ac468b` with all 3,090 paths; no integration file was edited by the campaign. The same diagnostic required the missing RegionRoute integration task referenced by externally added review knowledge; root added only that task record before retrying the gates.
- Knowledge: Gateway plugin-system/development guides, time conventions, log safety and canonical access-log guidance, plugin-runtime review precedents, unit-testing guidance, root AGENTS and graded design rules.
- Review: `b0131_final_review` independently approved the implementation with no Must Fix or Should Fix. It confirmed the helper is private, all and only the three runtime timing sites use it, timer boundaries and async waits are unchanged, and plugin order, identity restoration, log suppression, ExtensionRef omission, rejection behavior, body streaming, allocation, logging, metrics, and tracing remain unchanged. `b0131_prereq_review` independently approved the refreshed frozen manifest and RegionRoute deferred-test record after correcting their external-commit attribution.
- Verification: the focused future-Instant regression passed and the complete PluginRuntime module suite passed 37/37. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and refreshed 3,090-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: pending local batch commit; never push.
