# B0089 — Mock Deterministic Response Configuration

## Scope
- Baseline: `86a02dd4`; all analyzed target blobs were unchanged when the batch started
- Pipeline state: committed
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/mock/{mod.rs,plugin.rs}`
- Exact implementation files: `plugin.rs`, the shared `MockConfig` resource definition, and the standalone English and Chinese Mock guides
- Forbidden files: PluginRuntime, PluginSession, Controller, CRDs, shared header helpers, other plugins, every existing `edgion-tests/**` path, and protected user paths

## Context and value gate
- Mock accepted arbitrary configured header strings until runtime, where invalid values were silently dropped, case-insensitive duplicate names had order-dependent results, and operator-owned framing headers conflicted with the canonical response writer.
- Each request also rebuilt the delay duration and status log, while each generation allocated an owned plugin name and retained the complete resource configuration.
- Both guides incorrectly stated that non-terminating mode applied `statusCode` and `body` to a local response.
- Decision: make resource validation deterministic and framing-safe, prepare the immutable Gateway generation, and align both guides without changing the response lifecycle.

## Hard-rule boundary
- Preserve `100..=599` validation for D0046, the 30-second delay cap, delay-before-mutation order, custom-header-before-Content-Type order, and exact terminating status/body behavior.
- Preserve non-terminating behavior: queue headers and Content-Type, return `GoodNext`, and do not apply `statusCode` or `body` locally.
- Keep request-level observability exclusively in `PluginLog` and the canonical access log. Add no tracing, metric, body cache, failure-policy knob, or existing integration-test edit.
- Validation errors for header syntax and reserved names are static and never echo configured header values.

## Implementation and review
- Complexity: moderate; implementation started only after an independent plan review
- Analyst: `b0084_final_code_review`; implementer: `b0055_runtime_analysis`; plan reviewer: `b0084_plan_review`; final reviewer: `b0088_new_final_review`
- Resource change: validate status, delay, Content-Type value, all header names, all header values, reserved Content-Type/Content-Length/Transfer-Encoding names, and case-insensitive duplicates in deterministic classification passes
- Gateway change: retain only prepared generation fields, use a static name, and precompute the optional delay and terminating status log; the configured body still requires one request-time clone under the current owned `ErrResponse` API
- Regression coverage: resource syntax, length, sensitive-value redaction, mixed-case reserved names, duplicate names, deterministic error priority, generation independence, static identity, terminating/non-terminating results, exact logs, and paused-time header ordering
- Documentation: aligned both languages on header syntax, uniqueness, framing ownership, delay order, validation timing, and the exact non-terminating contract
- Final verdict: approved after strengthening the paused-time test to prove both header writes occur only after the delay and in custom-header-to-Content-Type order
- Deferred adjudication: D0055 owns the cross-plugin local-response body representation. Existing integration coverage remains frozen and is sufficient for this batch.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 5 Gateway Mock, 14 resource Mock, and 41 PluginRuntime tests |
| Independent reviews | passed | plan approved before implementation; final correction and re-review approved |
| Full workspace gates | passed | format, all-target check, and Clippy completed with only existing warnings |
| Repository guards | passed | agent docs, English-only, SSA, metrics, Pingora source, gateway no-tracing, diff, scope, and frozen paths |
| Frozen integration paths | passed | no existing integration file changed; no new integration issue was needed |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change owned row
- Commit: `ea6c94ba`
