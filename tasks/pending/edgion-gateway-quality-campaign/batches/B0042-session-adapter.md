# B0042 — Plugin Session Adapter

## Scope

- Baseline: `8d5c94823aa5ea934be1d9ac964426673ad9502d`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- File: `edgion-gateway/src/plugins/runtime/session_adapter.rs`.
- Allowed implementation file: `edgion-gateway/src/plugins/runtime/session_adapter.rs` only.
- Forbidden files: every source and knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Exact commit allow-list: the anchor file plus B0041/B0042 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `examined-no-change` after complete target review and ordered revalidation on the B0041 commit that touched its regression tests.
- Ownership evidence: `PingoraSessionAdapter` remains the canonical bridge for live Pingora request/response state, immutable request identity, terminal arbitration, bounded request-body handling, response streaming, diagnostics, dye rendering, and request-pinned ExtensionRef lookup.
- Performance evidence: hot consumers already use canonical borrowed header extensions; owned APIs remain where ownership must cross awaits. Backend ExtensionRef cache access is bounded to selected references and stages.
- Contract evidence: best-effort queued opaque response headers, body admission/replay, H1/H2/gRPC denial, redaction/access-log diagnostics, and immutable scope pinning match their current regression knowledge and tests.
- Rejected churn: splitting the large adapter without a demonstrated ownership or performance gain would increase lifecycle risk without material value.
- B0041 freshness: its exact PluginLog assertion strengthens the denial evidence and changes none of the adapter ownership conclusions.
- Independent review found one stale nearby comment that still called full response accumulation/replay a future P0 spike. A separate adjudicator confirmed this contradicts the permanent main-response streaming invariant and clears the value threshold for a comment-only correction.

## Implementation

- Replaced the obsolete P0 accumulate/replay comment with the permanent invariant: complete main-upstream-response reads are intentionally unsupported, the response remains streaming, and processing uses current chunks or the canonical bounded semantic-window streaming path.
- Kept `_cap`, `BodyReadError::Unimplemented`, and every runtime/test behavior unchanged.

## Independent review

- Initial independent review found no runtime defect but rejected the stale response-aggregation guidance. Independent adjudication confirmed the finding and approved a one-file, non-plan-first comment correction.
- A separate final reviewer approved the corrected wording against the body invariants and confirmed that it does not imply retained bytes alone provide backpressure.

## Verification

- Analysis and current-HEAD revalidation each passed all 122 focused session-adapter tests; implementation passed 9 focused response-body tests.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only. Only the campaign's known Kafka/frozen-test lint and duplicate-dependency warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `ab9a9e4f69f861ca9f23945c0912c09b2c31794e`.
