---
name: html-entity-decode-reference-semantics
description: Use when reviewing the native WAF htmlEntityDecode transformation, its reference boundaries, optional semicolon and name case rules.
knowledge_type: regression-guards
---

# htmlEntityDecode follows the libmodsecurity reference boundaries

## Conclusion and scope

`html_entity_decode` and `decode_entity_reference` in
`edgion-waf/src/seclang/transform.rs` decode a reference that starts at `&`:

- `&#` followed by the maximal run of ASCII decimal digits, or `&#x`/`&#X`
  followed by the maximal run of ASCII hex digits, is a numeric reference.
- Otherwise the maximal run of ASCII alphanumerics is a name. It decodes only
  when the whole run equals a table name ignoring ASCII case: `quot amp lt gt
  nbsp` (the libmodsecurity set) plus Edgion's `apos colon newline sol tab`.
  `&LT;`, `&Lt` and `&AMP;x` decode; `&ampx` and `&ltest;` do not.
- The terminating `;` is optional for both kinds and is consumed when it
  directly follows the run: `javascript&#58alert(1)` becomes
  `javascript:alert(1)`, `&#x3c` becomes `<`, `&#58;;` becomes `:;`.
- A reference with an empty run (`&`, `&;`, `&#`, `&#x`, `&#xZZ`) or an unknown
  name emits `&` and scanning resumes at the next byte, so the rest is copied.
- The value is emitted as UTF-8. A value that is not a Unicode scalar value,
  including a `u32` overflow, becomes U+FFFD.

The borrowed identity gate is unchanged: an input without `&` is a no-op.

## Rationale and evidence

CRS rules (941xxx XSS, 920/921/944) apply `t:htmlEntityDecode` and their
regression fixtures are validated against ModSecurity and Coraza. libmodsecurity
v3 `src/actions/transformations/html_entity_decode.cc` (read 2026-11-07)
consumes the longest digit run for `&#`/`&#x`, the longest `isalnum` run for a
name, compares names with `strcasecmp` on the whole run, and skips a `;` only
if it is present. Browsers likewise decode `&#58`, `&#x3c`, `&lt` and `&LT`
without `;`. Requiring `;` and exact-case names let
`<a href="javascript&#58alert(1)">` keep its literal form and miss 941170
(finding `waf-03`, re-confirmed by review finding F-S2-1).

## Alternatives and limits

libmodsecurity writes `(unsigned char)strtol(...)`, so `&#x13c;` becomes `<`
there and `C4 BC` here. Edgion keeps the browser-correct UTF-8 output; the
choice is tracked as `tasks/improve-issues/waf5-06-html-entity-decode-one-byte-truncation-divergence.md`.
On `u32` overflow the reference keeps a saturated `long` low byte while Edgion
emits U+FFFD (previously the overflowing reference was copied literally).
Edgion's extra HTML5 names are a superset of the reference and only decode
more. The full HTML5 named-reference table is not implemented.

## Verification and re-evaluation

`cargo test -p edgion-waf --lib html_entity` runs
`html_entity_references_follow_reference_semicolon_and_case_rules` (failed on
`javascript&#58alert(1)` before the 2026-11-07 fix) and
`html_entity_delimiters_preserve_decoding_and_output_limits` (its `&amp` row now
expects `&`). The CRS 4.25.1 FTW offline inventory after the change reported
`active_rules=567 admitted=4741 passed=4741 mismatched=0 negative_admitted=896
negative_mismatched=0`, identical to the baseline. Re-evaluate when the named
table, the numeric output encoding (waf5-06) or the reference version changes.

## Resolution references

- `waf-03` (fixed 2026-11-07): `htmlEntityDecode` required `;` and exact-case
  names; F-S2-1 in the 2026-11-07 transforms review confirmed it.
- F-S2-6 / `waf5-06`: one-byte truncation divergence, left open for an owner
  decision.
