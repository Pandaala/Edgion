# B0041 — Plugin Runtime

## Scope

- Baseline: `3782366f41a793862d75c117fd4b06e2729fb207`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Anchor file: `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`.
- Approved test support: `edgion-gateway/src/plugins/runtime/session_adapter.rs`.
- Forbidden files: every other source and knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Exact commit allow-list: the two approved Gateway files plus B0040/B0041 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `change`; simple and not plan-first.
- The response-stage naked `ErrTerminateRequest` fallback appends the literal `"deny terminate; "` to canonical PluginLog output. It is the sole repository occurrence and violates the canonical no-trailing-semicolon-space wording contract.
- Normalize the fixed token to `"deny terminate"` and pin the exact rendered output in the existing response-stage termination regression.
- Preserve response denial, termination arbitration, stage ordering, access-log ownership, redaction, schema/transport, body streaming, publication, and allocation behavior.
- The broader constructor, four-stage dispatch, ExtensionRef, validation parity, generation ownership, policy ordering, and body-boundary audit found no other justified change.

## Implementation

- Normalized the sole naked response-stage termination token from `"deny terminate; "` to `"deny terminate"`.
- Extended the existing adapter regression to retain the PluginLog and assert its exact serialized token.
- Preserved the existing HTTP 502 denial, termination-without-response diagnostic markers, final status, and response-stage control flow.

## Independent review

- An independent reviewer approved the two-file diff with no findings after checking the canonical PluginLog contract, denial semantics, allocations, observability ownership, redaction, body/schema boundaries, and frozen scope.
- The reviewer additionally passed the exact regression and all 67 record-outcome tests.

## Verification

- Focused verification: 53 PluginRuntime tests, the exact response-stage termination regression, 67 record-outcome tests, Gateway all-target check, formatting, no-tracing, English-only, and diff checks passed.
- Full repository gates: workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only passed; only the campaign's known Kafka/frozen-test lint and duplicate-dependency warnings remain.
- Frozen-path verification: all 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
