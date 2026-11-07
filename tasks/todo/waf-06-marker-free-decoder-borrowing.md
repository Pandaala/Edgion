# Borrow marker-free input through CssDecode, EscapeSeqDecode and JsDecode

Priority: P2   Area: plugin:waf (edgion-waf transformations)

## Problem
`walk_transformations` in `edgion-waf/src/seclang/transform.rs` (identity check
around line 150) already keeps the current input borrowed for `None`, unchanged
`Lowercase`, and marker-free `RemoveNulls` / `HtmlEntityDecode` / `UrlDecodeUni`.
`CssDecode`, `EscapeSeqDecode` and `JsDecode` (`css_decode`, `escape_sequence_decode`,
`js_decode`) copy every byte unchanged when the input contains no `\`, yet they
still go through the materializing path: `admit_scratch`, `output.clear()`,
`try_reserve_exact`, and a byte-by-byte `push_byte` copy.

CRS-style XSS rules chain `urlDecodeUni,htmlEntityDecode,jsDecode,cssDecode,...`
over every ARGS/cookie/header value, so for the overwhelming majority of inputs
(no backslash) each rule evaluation pays one or two needless full copies. Because
the chain stops being `Cow::Borrowed`, these results are also ineligible for the
existing `BorrowedTransformMemo` in `transaction.rs`
(`evaluate_single_input_with_transform_memo`), so repeated rules re-run the chain.

## Proposed change
Extend the existing identity match with the same gating pattern:
`current.len() <= limits.max_output_bytes && memchr::memchr(b'\\', current).is_none()`
for `CssDecode`, `EscapeSeqDecode` and `JsDecode`. No new cache, no new type;
the memo picks up the newly borrowed results automatically.

## Must preserve
- Byte-identical output for every input; marker-bearing inputs still decode
  through the current functions (the extra `memchr` prescan is the only cost).
- Per-step output caps, work charges (`work += current.len()`), checkpoint/observer
  order and `max_work_bytes` failure order, including multiMatch behaviour.
- Physical scratch admission changes are intended: skipping the destination means
  no new `admit_scratch` for that step (same as the existing identity cases);
  already retained scratch is not released.

## Verification
- Differential unit tests in `edgion-waf/tests/unit/seclang/transform_test.rs`:
  for each of the three decoders, marker-free vs marker-bearing inputs (short,
  long, at `max_output_bytes`, trailing lone `\`) give identical bytes and work.
- Assert borrowed result / memo hit for a marker-free CRS-like chain.
- `cargo test -p edgion-waf transform` and the WAF transaction tests.
