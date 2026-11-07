---
name: numeric-operators-non-numeric-operand-as-zero
description: Use when reviewing native WAF numeric operators (@eq, @ge, @gt, @lt), setvar += / -= arithmetic, or the integer conversion that can be reached by client input.
knowledge_type: regression-guards
---

# Numeric operators treat a non-numeric operand as zero

## Conclusion and scope

`parse_i64` in `edgion-waf/src/seclang/transaction/operators.rs` is the only
runtime integer conversion. It is total and follows ModSecurity v3, whose
ordering operators compare `atoll(input) <op> atoll(operand)` (glibc `atoll` is
`strtoll(s, NULL, 10)`); Edgion applies that `atoll` shape uniformly, `@eq`
included (see the `@eq` divergence below):

- Leading C-locale ASCII whitespace (space, `\t`, `\n`, `\v`, `\f`, `\r`) is
  skipped (`" 12"` is 12, `"\t-3x"` is -3). Other bytes, including non-ASCII
  whitespace, are not.
- An optional `+` or `-` followed by the longest leading run of ASCII decimal
  digits is the value. Bytes after the run are ignored (`"12abc"` is 12,
  `"1.9"` is 1).
- A value with no digits after the optional sign is 0 (`"abc"`, `""`, `" "`,
  `"-"`, `"--1"`, `"- 1"`).
- A digit run outside the `i64` range saturates at `i64::MAX` / `i64::MIN`, as
  glibc `strtoll` does (`LLONG_MAX` / `LLONG_MIN`).

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

ModSecurity v3 converts the operands per operator (checked against the
`v3/master` source on 2026-11-08):

- `@gt`, `@ge`, `@lt`, `@le` (`src/operators/gt.cc`, `ge.cc`, `lt.cc`,
  `le.cc`) use `atoll`: `"abc"` is 0, `"12abc"` is 12, `" 7"` is 7, and an
  out-of-range run saturates through glibc `strtoll`.
- `@eq` (`src/operators/eq.cc`) converts both sides with `std::stoi` inside its
  own `try`/`catch`: whitespace skip, sign and decimal prefix match `atoll`,
  but the range is C `int`, and a value with no digits or outside the `int`
  range becomes 0 instead of saturating.

Its `setvar` arithmetic treats an unconvertible value as 0. CRS is written
against that behavior; no CRS rule feeds `@eq` a client-controlled magnitude
outside the `int` range.

## Alternatives and limits

Coraza uses Go `strconv.Atoi` and ignores the error. It differs from
ModSecurity and Edgion on a partial prefix (`"12abc"` is 0 there, 12 here) and
on leading whitespace (`" 7"` is 0 there, 7 here); it clamps an out-of-range
run like `strtoll`. The references agree on fully numeric and fully
non-numeric values. Edgion follows ModSecurity v3 `atoll` for all four
operators. Pinned divergence: for `@eq` with a value outside the 32-bit `int`
range, ModSecurity yields 0 (`n=99999999999` matches `@eq 0` there) while
Edgion compares the saturated `i64` (no match). This is result-visible only and
cannot hide a payload; one total, saturating conversion shared by every
operator is the deliberate choice. Saturation depends
on glibc `strtoll`; a libc without saturation would make the reference result
implementation-defined, and Edgion pins saturation. Keeping the strict
conversion as a fatal error was rejected because one client header turns into a
transaction abort; a "no match" result for unconvertible values was rejected
because it diverges from both references for `"abc" @lt 5`.

## Verification and re-evaluation

`cargo test -p edgion-waf --lib` runs
`signed_integer_conversion_skips_leading_whitespace_reads_the_decimal_prefix_and_saturates`,
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
- P6 review follow-up (2026-11-08): the guard previously attributed the
  conversion to ModSecurity `atoi` without whitespace skipping. ModSecurity v3
  uses `atoll`, so `parse_i64` now skips leading ASCII whitespace and this page
  states the `atoll` / `strtoll` semantics.
- P7 review follow-up (2026-11-08): `@eq` is `std::stoi` in ModSecurity v3,
  not `atoll`; the page now states the per-operator conversion and pins the
  `@eq` out-of-`int`-range divergence.
