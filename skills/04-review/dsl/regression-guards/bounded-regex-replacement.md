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
move its owned original `Arc<str>` and AI Guard to leave the span unchanged.

The bound applies to the helper-owned output's logical length and its requested
growth. ARCH-030 replaces exact per-prefix growth with a simple doubling policy:
after admitting the next prefix, reserve only when existing capacity is insufficient,
targeting `max(next_length, current_capacity.saturating_mul(2)).min(limit)`.
Reservation remains fallible and requests the target minus current length. A large
append fits in one request; smaller appends reuse spare capacity. The helper does
not eagerly reserve the entire caller limit.

Spare capacity increases retained memory, and requesting a larger target can fail
under memory pressure where the old exact-prefix request might succeed. Such a
failure retains the existing `AllocationFailed` variant and consumer mappings.
AI Guard still charges logical replacement lengths across spans and chained rules,
and separately validates the final composed logical total; these ledgers are not
heap limits. Chained rewriting temporarily retains old and new strings together.
For a positive local cap B, requested growth targets are below twice the admitted
prefix and at most B. Under an exact-grant model, retained local replacement
targets sum to less than twice the aggregate logical cap, and the old set plus one
candidate has a conservative bound below four times that cap. These are requested
capacity models, not allocator-grant or RSS ceilings. Zero-byte outputs allocate
no builder storage. The cap does not claim that regex-engine scratch space is absent, that an
allocator will not round capacity, that interpolation-token storage is covered
by the output cap, that source text is covered, or that conversion to `Arc<str>`
allocates nothing.

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

ARCH-030 adds small-append capacity-transition regressions, target arithmetic
including saturation/clamping and large appends, unchanged output/capacity for
rejected prefixes, and zero-byte construction. Capacity checks permit allocator
rounding and do not assert exact grants. No allocation benchmark, throughput
measurement or process-memory measurement is claimed.

Re-run the differential grammar and zero-width matrix whenever the resolved
`regex` or `regex-automata` version changes. This guard does not govern regex
compilation size limits, request-body scanning, literal body replacement, or
main upstream response streaming.
