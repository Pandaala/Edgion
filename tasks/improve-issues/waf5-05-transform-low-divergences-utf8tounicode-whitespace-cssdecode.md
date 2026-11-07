# waf5-05: Low-severity transformation divergences from ModSecurity (`utf8toUnicode` surrogates, whitespace `0x85`, `cssDecode` long escapes, `urlDecodeUni` `%U`)

- **Category**: best-practice
- **Severity**: Low (F-S2-2) / Info (F-S2-3, F-S2-4, F-S2-5)
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/transform.rs:864-897` (`utf8_to_unicode`; `std::str::from_utf8` at `:879` rejects UTF-8-encoded surrogates)
  - `edgion-waf/src/seclang/transform.rs:467-469` (`is_latin_space` includes `0x85`), used by `compress_whitespace` (`:471`) and `RemoveWhitespace` (`:361`)
  - `edgion-waf/src/seclang/transform.rs:487-528` (`css_decode`; full-width fold on the whole accumulated value at `:517-520`)
  - `edgion-waf/src/seclang/transform.rs:832-862` (`url_decode_uni`; `matches!(input[index + 1], b'u' | b'U')` at `:839`)

## Summary
The 2026-11-07 transformation lens found four divergences from ModSecurity v3 / Coraza besides the fixed
`htmlEntityDecode` defect (waf-03, commit `2ac1fc9c8`) and the separate waf5-06. None was shown to bypass a
CRS 4.25.1 rule; three are in the over-decoding (false-positive) direction. They are recorded so that
differential runs (go-ftw, Coraza comparisons) are not mis-triaged and so the owner can decide parity versus
documented deviation.

## Current behavior (evidence)
| ID | Input (bytes) | Edgion | ModSecurity v3 / Coraza | Direction |
|---|---|---|---|---|
| F-S2-2 | `ED A0 80` (UTF-8-encoded U+D800) under `t:utf8toUnicode` | three bytes copied raw | `%ud800` (no surrogate check) | differs; no CRS bypass found (neither form matches CRS `\s`/`\b` patterns) |
| F-S2-3 | `a 85 b` under `t:compressWhitespace` / `t:removeWhitespace` | `0x85` treated as whitespace | `isspace()` + `0xA0` only | Edgion matches more; `0x85` is also a UTF-8 continuation byte, so a multi-byte character can be split |
| F-S2-4 | `\10ff3c` / `\1ff1c` under `t:cssDecode` | `<` / `0x1c` (fold on the full 24-bit value) | `\` / `<` (5-6 digit escapes use the last four digits) | both differ from CSS; no CRS case uses 5-6 digit escapes |
| F-S2-5 | `%U003c` under `t:urlDecodeUni` | decoded to `<` | only lowercase `%u` | superset (matches IIS); can only add matches |

CRS 4.25.1 uses all four transformations; the full differential table and per-row confidence are in the
review evidence page `skills/04-review/waf/review-evidence/known-bypass-class-audit-2026-11.md`.

## Reference behavior
ModSecurity v3 `src/actions/transformations/{utf8_to_unicode,compress_whitespace,remove_whitespace,css_decode,url_decode_uni}.cc`
and Coraza `internal/transformations/*.go`. Confidence (S2): F-S2-2 medium, F-S2-3 high, F-S2-4 medium,
F-S2-5 medium.

## Suggested fix
Decision for the owner, per row: adopt reference parity (emit `%u` for surrogates; drop `0x85` from
`is_latin_space`; fold on the last four digits for 5-6 digit CSS escapes; accept only lowercase `%u`) or
record each as an intentional deviation in an `intentional-design` page. Any parity change gets a
`transform_test.rs` case with the byte input written as an ASCII escape, e.g.
`utf8_to_unicode_surrogate_bytes_follow_reference`, `compress_whitespace_matches_reference_byte_set`,
`css_decode_long_escape_matches_reference`.

## Notes
- Review input: findings F-S2-2..F-S2-5 (2026-11-07 second-pass transforms lens). F-S2-1 was fixed as
  waf-03; F-S2-6 is waf5-06.
