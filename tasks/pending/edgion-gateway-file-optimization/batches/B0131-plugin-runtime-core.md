# B0131 — Plugin Runtime Core

## Scope

- Baseline: `b71f37f2fe8b3ae837bcfb08e159d540a0b97715`
- Pipeline state: analysis-ready
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

- Analysis: `b0131_plugin_runtime_analysis` read all 3,643 lines and traced constructors, four stage dispatchers, condition/body/dye wrapping, immutable publication, HTTP/gRPC consumers, and existing tests/review precedent. Decision: change; complex because three request-path timing loops are touched.
- Value: replace three `Instant::elapsed().as_micros() as u64` expressions with one private checked, saturating microsecond helper, following the WAF precedent and canonical time rules. Preserve normal timing values, log omission, plugin order, rejection behavior, and streaming; add a deterministic future-Instant regression. No new allocation or observability.
- Deferred: the existing route/backend response-body-dispatch task and D0081/D0082/D0089 retain their scopes; no new issue or integration test is required.
- Later queued evidence: B0132 raised D0095, a not-yet-reproduced policy/invalid-generation retention interaction. Keep that publication investigation separate from this proposed timing-only change and revalidate the boundary on promotion.
- Knowledge: Gateway plugin-system/development guides, time conventions, log safety and canonical access-log guidance, plugin-runtime review precedents, unit-testing guidance, root AGENTS and graded design rules.
- Review: pending.
- Verification: pending.
- Commit: pending.
