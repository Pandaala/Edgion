# SSE Window Strict Residency and Continuity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

`RawWindow` needs a separately designed fix for semantic continuity and strict transient-memory accounting. The defects are confirmed, but the first implementation attempt grew to more than 900 added lines and still undercounted hidden allocations. Do not patch these findings incrementally. Start with a small peak-residency model that makes every allocation visible before construction, then implement and review the complete lifecycle together.

## Requirements

- Preserve overlap only from bytes actually emitted. Semantic replacement must derive the next overlap from prior emitted context plus replacement text; discard must preserve prior emitted context; opaque raw replacement must reset or explicitly reject semantic continuity.
- Terminal semantic replacement must not construct, charge, or install overlap that is immediately discarded.
- Enforce the combined-residency cap before allocations occur. The ledger must include raw storage, overlaps, metadata, materialized output, raw token scratch, decoded JSON text, replacement inputs, edit descriptors, encoded output, and staged bucket updates.
- Decode JSON string tokens without hidden `serde_json` scratch ownership in the hard-bound path. Escapes and UTF-16 surrogate pairs must retain exact JSON semantics.
- Avoid allocating stable-sort scratch and avoid repeated linear bucket-to-replacement lookup. Bounded fixed-index plans or allocation-free ordering are preferred.
- Bucket append planning must not clone accumulated buckets, fragments, or range vectors. Failure must leave the prior generation byte-identically releasable.
- Charge the existing raw handle table once. Append plans must expose only genuinely additional or simultaneously live metadata.

## Confirmed findings

1. Semantic replacement previously calculated overlap from replacement text alone, losing prior emitted context; discard cleared valid prior overlap.
2. Semantic materialization used `serde_json::from_slice::<String>`, whose internal decoder scratch and returned `String` can coexist with the raw scratch and output but were not all charged.
3. Replacement and bucket plans can allocate sort/grouping/edit storage before the combined-residency rejection point.
4. Nonterminal opaque `WindowDecision::Replace` can preserve overlap from original semantic bytes that were not emitted.
5. The common `AppendSlices` accounting path can double-charge the already-live raw handle table.
6. Repeated full bucket cloning and repeated linear replacement lookup create avoidable quadratic work.

## Rejected attempt

The 2026-08-30 audit produced a candidate of approximately `+921/-192` lines in `sse/window.rs`. Although its focused tests passed, independent review rejected it because it still had hidden allocation peaks, useless terminal overlap, opaque-replacement semantic drift, metadata double charging, and nested linear lookup. A second smaller attempt was stopped before completion under the complexity policy. Neither candidate was committed.

## Acceptance tests

- Three generations: release → small semantic replacement → next retain preserves the fair UTF-8-safe suffix of emitted bytes.
- Three generations: release → discard → next retain preserves only the previously emitted suffix.
- Opaque raw replacement with semantic buckets cannot expose original, non-emitted text as next-generation context.
- Terminal semantic replacement succeeds when the cap fits output/edit storage but not an unnecessary overlap.
- Escaped JSON tokens, including valid and invalid surrogate pairs, have exact-cap and cap-minus-one peak tests with no hidden decoder allocation.
- Existing raw handle-table append succeeds at the exact additional peak and fails atomically at one byte below it.
- Repeated same-bucket retain preserves existing range-allocation identity; repeated replacement lookup is linear or bounded direct-index work.
- Full `cargo test -p edgion-gateway --lib sse::` and the guardrail response tests pass, followed by the normal workspace guards.

## Scope

### In scope

- `edgion-gateway/src/sse/window.rs`
- The narrow guardrail EOS preview call site if a terminal-aware API is required
- A minimal `ChainAppendPlan` accounting accessor only if the existing plan cannot expose additional metadata accurately

### Out of scope

- Response aggregation or response-side `BodyStorage`
- Changes to SSE wire framing in `sse/mod.rs`
- ExtProc STREAMED semantic reinterpretation
- Existing integration-test files

## Related context

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/00-overview.md` — canonical SSE substrate
- `AGENTS.md` — HTTP body handling invariants
- `tasks/pending/edgion-gateway-file-optimization/`
