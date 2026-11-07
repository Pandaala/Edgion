---
name: numeric-operators-non-numeric-operand-as-zero
description: Use when reviewing native WAF numeric operators (@eq, @ge, @gt, @lt), setvar += / -= arithmetic, or the integer conversion that can be reached by client input.
knowledge_type: regression-guards
---

# Numeric operators treat a non-numeric operand as zero

## Conclusion and scope

`parse_i64` in `edgion-waf/src/seclang/transaction/operators.rs` is the only
runtime integer conversion. It is total and follows ModSecurity's `atoi`:

- An optional `+` or `-` followed by the longest leading run of ASCII decimal
  digits is the value. Bytes after the run are ignored (`"12abc"` is 12,
  `"1.9"` is 1).
- A value with no digits after the optional sign is 0 (`"abc"`, `""`, `"-"`,
  `"--1"`, `" 7"`; leading whitespace is not skipped).
- A digit run outside the `i64` range saturates at `i64::MAX` / `i64::MIN`.

It is used for both sides of `@eq`, `@ge`, `@gt` and `@lt` (the input value and
a macro-expanded operand, `evaluate_operator`), for the stored TX value and the
macro-expanded delta of `setvar:tx.x=+N` / `=-N` (`apply_member_actions` in
`transaction/units.rs`; a missing TX value is also 0), and for the static TX
gates of fused JSON/XML inspection (`json_inspection_collector.rs`), so the
gate predicts exactly what the runtime evaluates. `setvar` `+=` / `-=` results
saturate. No numeric conversion produces a `RuntimeError`; the former
`RuntimeError::IntegerConversion` variant was removed.

Compile-time constants keep their strict grammar: `strict_i64` in
`semantic/operand.rs` rejects `@eq abc` or an out-of-range `setvar:tx.x=+N`
constant when the rule set is compiled. That is an operator-authored input,
not request data.

## Rationale and evidence

Numeric operands are frequently client-controlled, directly (`ARGS:x "@gt 10"`)
or through a capture. CRS 4.25.1 rule 920190 (PL1, shipped in the supported
core bundle) captures two unbounded `\d+` runs from `Range` and evaluates
`TX:2 "@lt %{tx.1}"`. With the former strict `str::parse::<i64>`,
`Range: bytes=99999999999999999999-1` failed the request-headers phase and the
gateway answered 500, in `DetectionOnly` too (finding `waf3-01`). The same
applied to `setvar` arithmetic on a captured or stored non-numeric value, and
to CRS 921180's `TX:/paramcounter_.*/ "@gt 1"`, which also selects the
920450 header-name entries whose values are not numbers.

ModSecurity v3 converts these operands with C `atoi`-style prefix parsing
(`"abc"` is 0, `"12abc"` is 12) and its `setvar` arithmetic treats an
unconvertible value as 0. CRS is written against that behavior.

## Alternatives and limits

Coraza uses Go `strconv.Atoi` and ignores the error, so `"12abc"` is 0 there
and 12 here; the two references agree on fully numeric and fully non-numeric
values and differ only on a partial prefix. Edgion follows ModSecurity.
ModSecurity's `atoi` also skips leading whitespace and its overflow is
undefined; Edgion does not skip whitespace and saturates. Keeping the strict
conversion as a fatal error was rejected because one client header turns into a
transaction abort; a "no match" result for unconvertible values was rejected
because it diverges from both references for `"abc" @lt 5`.

## Verification and re-evaluation

`cargo test -p edgion-waf --lib` runs
`signed_integer_conversion_reads_the_leading_decimal_prefix_and_saturates`,
`numeric_operator_on_unbounded_range_capture_saturates_instead_of_failing`
(the 920190 shape; failed with `IntegerConversion` before the change),
`numeric_operators_convert_both_operands_by_leading_decimal_prefix`,
`setvar_arithmetic_treats_non_numeric_values_as_zero_and_saturates` and
`json_and_query_numeric_operands_convert_non_numeric_values_to_zero`.
Re-evaluate if a new integer operator (`@le`, `@ne`) or another runtime integer
conversion is added: it must reuse `parse_i64` rather than a strict parse.

## Resolution references

- `waf3-01` (decision commit, 2026-11-07): numeric operators and setvar
  arithmetic aborted the transaction on client-controlled non-`i64` input;
  F-S5-2 (setvar overflow) is the same root cause.
