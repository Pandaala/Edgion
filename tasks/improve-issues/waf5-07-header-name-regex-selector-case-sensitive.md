# waf5-07: `REQUEST_HEADERS:/regex/` selectors match header names case-sensitively, while literal header selectors are ASCII-case-insensitive

- **Category**: correctness
- **Severity**: Low
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/compiler/lowering.rs:538-551` (`ascii_case_insensitive = target.collection() == CollectionKind::Tx`; every other collection compiles the selector regex case-sensitively)
  - `edgion-waf/src/seclang/transaction/selection.rs:583-601` (literal selectors: `eq_ignore_ascii_case` for `REQUEST_HEADERS`, `REQUEST_HEADERS_NAMES`, `RESPONSE_HEADERS`, `TX`)
  - `edgion-waf/src/seclang/transaction/selection.rs:633-644` (regex selectors: `regex.is_match(name, ...)` on the name as stored)
  - pinned by `edgion-waf/tests/unit/seclang/transaction/mod_test.rs::direct_tx_selection_matches_the_generic_cursor_order_counts_and_exclusions` (`REQUEST_HEADERS:/^x-d/` matches `x-dup` but not `X-Dup`)

## Summary
For header collections Edgion treats names case-insensitively when the selector is a literal and
case-sensitively when it is a regex. Inside the engine the header name is stored with the case the host
supplied. Through the gateway, names arrive lowercased from the `http` crate, so a regex selector written
with uppercase letters (for example `REQUEST_HEADERS:/^X-Forwarded-/` or an exclusion
`!REQUEST_HEADERS:/^X-Api-/`) silently never matches, while the same name as a literal would.
A host that passes mixed-case names (unit tests, a future non-`http` caller) sees the converse: `/^x-d/`
misses `X-Dup`.

## Current behavior (evidence)
Observed while T12c (`9ddd70ff9`, direct TX walk) pinned selection order: with headers `X-Dup`, `x-dup`,
`X-Dz`, `SecRule REQUEST_HEADERS:/^x-d/|!REQUEST_HEADERS:X-DZ` yields only `REQUEST_HEADERS:x-dup` in
`MATCHED_VARS`, whereas `&REQUEST_HEADERS:x-dup` counts 2 (literal, case-insensitive). `TX:/^V_/` is
case-insensitive because TX is the only collection compiled with
`compile_ascii_case_insensitive_boolean_regex`.

## Reference behavior
ModSecurity v3 matches literal header selectors case-insensitively. For regex selectors the brief for this
review states that ModSecurity applies the regex to the header name as stored; my recollection of
libmodsecurity v3 is that variable-key regexes (`VariableRegex`) are compiled with case-insensitive
matching, and Coraza stores header names lowercased. Confidence: low-medium; verify against
`src/variables/variable.h` and `src/anchored_set_variable.cc` before changing behavior. In either reading,
the gateway's lowercase names make uppercase regex selectors dead in Edgion, which is the practical risk.

## Suggested fix
Compile regex selectors for `REQUEST_HEADERS`, `REQUEST_HEADERS_NAMES` and `RESPONSE_HEADERS` with the same
ASCII-case-insensitive builder used for TX (one-line condition change in `lowering.rs:538`), so literal and
regex selectors agree; update the pinned test above. Alternatively reject or warn at compile time on an
uppercase ASCII letter in a header-collection regex selector.
Proposed regression test: `header_regex_selector_is_ascii_case_insensitive` in
`edgion-waf/tests/unit/seclang/transaction/mod_test.rs` - `REQUEST_HEADERS:/^X-D/` matches `x-dup`.

## Notes
- Review input: observed during T12c of the 2026-11-07 second pass; not raised by the S1-S5 lenses.
