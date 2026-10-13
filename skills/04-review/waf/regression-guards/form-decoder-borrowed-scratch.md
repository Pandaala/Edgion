---
name: form-decoder-borrowed-scratch
description: Preserve tolerant form-byte decoding and owned publication while borrowing unchanged decoder scratch.
knowledge_type: regression-guards
---

# Form decoding borrows unchanged scratch

ARCH-226 removes disposable decoder allocations from plain form names and values.
The private `percent_decode` in `edgion-waf/src/seclang/body.rs` returns borrowed
bytes when neither `+` nor `%` occurs. Marker-bearing fields use the existing
fallibly reserved decoder and return owned bytes. The preflight adds inspection
work for encoded fields; this is not a claim that every input becomes faster.

`parse_urlencoded` still splits segments before decoding, splits only the first
`=`, skips empty segments and preserves duplicates/order. Plus, percent hex case,
one-pass decoding, malformed/truncated literal escapes and arbitrary binary bytes
retain their tolerant semantics. `BodyBuilder::push` owns the final fallible copies
for ARGS and ARGS_NAMES; no decoder borrow survives publication.

The segment work debit still precedes decoding. Entry count, name/decoded byte
bounds, combined-size accounting, descriptor admission and final copies are
unchanged. Eliminating a temporary allocation eliminates only its potential
allocation failure. Body processor selection, canonical request storage and
transaction phase ownership are unaffected.

Private decoder tests cover Cow ownership, borrowed pointer identity and byte
semantics. Publication tests inspect fields after source release; existing literal
accounting tests exercise exact/one-less entry, byte and work limits, and transaction
tests publish real forms into Phase 2. These are local tests and source evidence,
not throughput, RSS or live-traffic measurements.

Re-evaluate when decoding semantics, work accounting, retained entry ownership or
publication limits change. Do not turn this private intermediate into source-backed
entries, a shared body cache or a generic strict URI decoder.
