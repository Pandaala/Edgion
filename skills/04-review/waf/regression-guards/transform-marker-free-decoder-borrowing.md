---
name: transform-marker-free-decoder-borrowing
description: Use when reviewing walk_transformations identity handling for CssDecode, EscapeSeqDecode and JsDecode on backslash-free input.
knowledge_type: regression-guards
---

# Marker-free backslash decoders stay borrowed

## Conclusion and scope

The identity gate in `walk_transformations`
(`edgion-waf/src/seclang/transform.rs`) covers `CssDecode`, `EscapeSeqDecode`
and `JsDecode`. Under the existing `current.len() <= limits.max_output_bytes`
guard, an input with no `\` byte is treated as an identity step, exactly like
marker-free `RemoveNulls`, `HtmlEntityDecode` and `UrlDecodeUni`. CRS-style XSS
chains (`urlDecodeUni,htmlEntityDecode,jsDecode,cssDecode`) therefore stay
`Cow::Borrowed` for ordinary values and become eligible for the existing
`BorrowedTransformMemo` in `transaction.rs`. No new cache or type was added.

## Rationale and evidence

`css_decode`, `escape_sequence_decode` and `js_decode` act only on a `\`
escape; every other byte is pushed to the output unchanged, so a backslash-free
input decodes to itself. The `memchr` prescan is the only extra cost on
marker-bearing inputs, which still run the unchanged decoder functions.

Preserved invariants:

- Work charges are identical: an identity step still charges `input.len()` at
  `StepInput` and `output.len()` (equal to the input) at `StepOutput`, so the
  total is the pre-change `input.len() + output.len()`; `max_work_bytes` and
  `max_output_bytes` failures keep their order.
- Checkpoint order and `stage` callbacks are unchanged (the identity branch
  shares the pre-existing emission code; order itself is not asserted by a
  test). Identity steps report
  `changed == false` and skip only the destination `admit_scratch` /
  `scratch_capacity` callbacks and the reservation they guard, as the existing
  identity arms already did; already retained scratch is not released. On the
  JSON inspection path this means a tight peak budget no longer rejects a
  backslash-free value at these steps.
- Under `multiMatch` an identity step never stages and never advances the stage
  ordinal; a marker-free `jsDecode,cssDecode` chain stages only `stage(0, input)`.
- Any `\`, including a trailing lone `\`, takes the decode path. `css_decode`
  drops a trailing lone `\`; `js_decode` and `escape_sequence_decode` keep it;
  bytes equal the pre-change output.
- The `current.len() <= max_output_bytes` clause is kept for uniformity with
  the existing arms; it is unreachable in practice because the walk rejects
  over-long input before the first step and every decoder output is capped.

## Alternatives and limits

Rejected: an early return inside the three decoders. It would still pay
`admit_scratch` and the reservation and would produce an owned result the memo
cannot reuse. This is a request-path cleanup, not a throughput or RSS claim.

## Verification and re-evaluation

`edgion-waf/tests/unit/seclang/transform_test.rs` covers borrowed results for
short, 4096-byte and exactly `max_output_bytes` inputs, differential bytes and
`work` for marker-bearing inputs against the decoder functions, identity work
and work-limit boundaries, and multiMatch non-staging.
`edgion-waf/tests/unit/seclang/transaction_test.rs`
(`borrowed_transform_memo_reuses_marker_free_crs_xss_decoder_chain`) asserts the
CRS-like chain publishes the memo key only for a marker-free value (a memo hit
versus a republish is not separately observable). Removing the
gate arm makes four of the five tests fail. These are local unit tests, not
throughput or live-traffic measurements.

Re-evaluate when any of the three decoders gains a transformation triggered by a
byte other than `\`, when a new transformation is added to the identity gate,
when step work accounting or checkpoint order changes, or when the
`BorrowedTransformMemo` key shape or eligibility rules change.

## Resolution references

- Former `tasks/todo/waf-06-marker-free-decoder-borrowing.md`, completed
  2026-11-07.
