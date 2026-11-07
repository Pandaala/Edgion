# waf5-06: `t:htmlEntityDecode` emits UTF-8 for code points above 0xFF; libmodsecurity truncates to one byte

- **Category**: best-practice
- **Severity**: Info
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/transform.rs` (`html_entity_decode`, `append_codepoint`)
  - reference: libmodsecurity v3 `src/actions/transformations/html_entity_decode.cc` (checked 2026-11-07), `*d++ = (unsigned char)strtol(...)`

## Summary
Edgion and libmodsecurity agree on which bytes form an HTML character reference
(optional `;`, maximal digit/alphanumeric run, case-insensitive names; see
`skills/04-review/waf/regression-guards/html-entity-decode-reference-semantics.md`).
They disagree on the output of a numeric reference whose value is above 0xFF.
libmodsecurity writes the low byte of the `strtol` result; Edgion writes the
UTF-8 encoding of the full code point (U+FFFD when the value is not a Unicode
scalar value or overflows `u32`).

## Current behavior
| input | libmodsecurity v3 | Edgion | browser |
|---|---|---|---|
| `&#x13c;` | `<` (0x3C) | `C4 BC` (U+013C) | U+013C |
| `&#x110000;` | `0x00` | `EF BF BD` (U+FFFD) | U+FFFD |
| `&#4294967354;` | `:` (low byte of the `long`) | `EF BF BD` | U+FFFD |

The reference over-decodes: a payload such as `&#x13c;script>` becomes
`<script>` in ModSecurity, while a browser renders a Latin letter. Edgion is
browser-correct. Bypass direction for Edgion is therefore limited to rules
whose CRS regression fixtures rely on the truncation; the CRS 4.25.1 FTW
inventory (567 active rules, 4741 admitted, 0 mismatched on 2026-11-07) does
not.

## Suggested fix
Decision for the owner: keep browser-correct UTF-8 output (current), or add the
reference's one-byte cast so CRS regexes see the same bytes as ModSecurity and
Coraza. If the cast is adopted, update the guard page and the
`&#x13c;`/`&#x110000;` rows in
`html_entity_references_follow_reference_semicolon_and_case_rules`. Review
input: review finding F-S2-6 (2026-11-07 second-pass transforms lens).
