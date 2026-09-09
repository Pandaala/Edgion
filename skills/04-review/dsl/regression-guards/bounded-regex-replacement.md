---
name: bounded-regex-replacement
description: Use when reviewing DSL or AI Guard regex replacement construction, interpolation grammar, literal fast paths, or output-limit enforcement.
knowledge_type: regression-guards
---

# DSL and AI Guard regex replacement output is bounded during construction

## Trigger

Before B0253, DSL called `Regex::replace_all` and checked `max_string_len` only
after the complete result had been allocated. A legal dynamic template with
repeated capture references could therefore amplify a bounded input far beyond
the sandbox limit before returning `StringTooLong`. AI Guard previously needed
the same correction for `maxReplacementBytes` in B0237.

## Correct behavior

`plugins/http/common/bounded_regex_replacement.rs` is the canonical mechanism
for both consumers. A replacement containing no `$` is exactly classified as
literal and uses `find_iter`; any replacement containing `$` uses the regex
1.13 capture-interpolation grammar and `captures_iter`. Numeric and named
references, braced references, longest unbraced names, `$$`, unknown captures,
malformed dollars, UTF-8 capture names, and zero-width matches must remain
differentially equal to `regex::Regex::replace_all`.

Each unchanged slice, literal, and capture expansion is checked before it is
appended. The inclusive caller limit accepts an exact-boundary result and
rejects the first crossing prefix. Checked length overflow reports
`usize::MAX`. A no-match call returns no helper-owned output, allowing DSL to
clone its original `Arc<str>` and AI Guard to leave the span unchanged.

The bound applies to the helper-owned output's logical length and its requested
growth. It does not claim that regex-engine scratch space is absent, that an
allocator will not round capacity, that interpolation-token storage is covered
by the output cap, or that conversion to `Arc<str>` allocates nothing.

## Consumer-specific errors

DSL maps output-limit failure to `RuntimeError::StringTooLong` with the first
crossing prefix length. Helper output-reservation and impossible iterator
invariant failures map to fixed `RuntimeError::Internal` messages without
input-derived detail, then follow the existing DSL error policy. AI Guard maps
all helper failures to its existing `Err(())` path and retains aggregate live
replacement accounting across spans and chained rules.

## Regression evidence and limits

Common-helper tests lock exact literal/capture branch classification, regex
1.13 replacement grammar, zero-width behavior, no-match behavior, exact and
plus-one limits, repeated-capture amplification, the first crossing prefix,
and checked-add overflow. DSL tests cover dynamic templates and typed
`StringTooLong`; AI Guard tests retain aggregate and chained accounting.

Re-run the differential grammar and zero-width matrix whenever the resolved
`regex` or `regex-automata` version changes. This guard does not govern regex
compilation size limits, request-body scanning, literal body replacement, or
main upstream response streaming.
