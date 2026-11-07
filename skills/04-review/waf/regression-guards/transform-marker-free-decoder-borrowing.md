---
name: transform-marker-free-decoder-borrowing
description: Keep backslash-free input borrowed through CssDecode, EscapeSeqDecode and JsDecode without changing bytes, work charges or multiMatch staging.
knowledge_type: regression-guards
---

# Marker-free backslash decoders stay borrowed

waf-06 extends the identity gate in `walk_transformations`
(`edgion-waf/src/seclang/transform.rs`) to `CssDecode`, `EscapeSeqDecode` and
`JsDecode`. Under the existing `current.len() <= limits.max_output_bytes` guard,
an input with no `\` byte is treated as an identity step, exactly like
marker-free `RemoveNulls`, `HtmlEntityDecode` and `UrlDecodeUni`. CRS-style XSS
chains (`urlDecodeUni,htmlEntityDecode,jsDecode,cssDecode`) therefore stay
`Cow::Borrowed` for ordinary values and become eligible for the existing
`BorrowedTransformMemo` in `transaction.rs`; no new cache or type was added.

Evidence: `css_decode`, `escape_sequence_decode` and `js_decode` only act on a
`\` escape; every other byte is pushed to the output unchanged, so a backslash-free
input decodes to itself. The `memchr` prescan is the only extra cost on
marker-bearing inputs, which still run the unchanged decoder functions.

Preserved invariants:

- Work charges are identical: an identity step still charges `input.len()` at
  `StepInput` and `output.len()` (equal to the input) at `StepOutput`, so the
  total is the pre-change `input.len() + output.len()`; `max_work_bytes` and
  `max_output_bytes` failures keep their order.
- Checkpoint order and observer callbacks are unchanged. Identity steps report
  `changed == false` and skip only destination admission and reservation; already
  retained scratch is not released.
- Under `multiMatch` an identity step never stages and never advances the stage
  ordinal; a marker-free `jsDecode,cssDecode` chain stages only `stage(0, input)`.
- Any `\`, including a trailing lone `\`, takes the decode path. `css_decode`
  drops a trailing lone `\`; `js_decode` and `escape_sequence_decode` keep it;
  bytes equal the pre-change output.
- Inputs longer than `max_output_bytes` never take the identity shortcut.

Tests: `transform_test.rs` covers borrowed results for short, 4096-byte and exactly
`max_output_bytes` inputs, differential bytes/work for marker-bearing inputs,
identity work and work-limit boundaries, and multiMatch non-staging.
`transaction_test.rs` (`borrowed_transform_memo_reuses_marker_free_crs_xss_decoder_chain`)
asserts the CRS-like chain publishes the memo key only for a marker-free value.
These are local unit tests, not throughput or live-traffic measurements.

Re-evaluate when any of the three decoders gains a transformation triggered by a
byte other than `\`, when a new transformation is added to the identity gate,
when step work accounting or checkpoint order changes, or when the
`BorrowedTransformMemo` key shape or eligibility rules change.
