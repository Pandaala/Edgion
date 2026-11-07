# waf5-04: Multipart `Content-Disposition` parsing diverges from RFC unescaping, ignores `filename*=` and lets a later header win

- **Category**: correctness
- **Severity**: Low
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/body.rs:767-813` (`disposition_parameters`: strips only a matched pair of outer `"`, keeps backslash escapes and an unmatched leading quote; recognizes `name` and `filename` only, `:792-796`)
  - `edgion-waf/src/seclang/body.rs:672-674` (`parse_multipart_part`: each `content-disposition` header line reassigns `(field_name, filename)`, so the last one wins)
  - pinned by `edgion-waf/tests/unit/seclang/body_test.rs::multipart_disposition_preserves_first_values_and_tolerant_grammar` and `::multipart_later_disposition_replaces_missing_filename`

## Summary
Three parameter-parsing differentials can make Edgion and the application bind a multipart part to
different names. A rule or CRS runtime exclusion scoped to `ARGS:<name>` (`ctl:ruleRemoveTargetById ...
ARGS:name`) then applies on one side only, and a file part can be seen as an ordinary argument or not at all.

## Current behavior (evidence)
1. Quoted-string escapes (F-S3-4): `Content-Disposition: form-data; name="p\";x"` yields the Edgion argument
   name `p\";x` (backslash retained); an RFC quoted-string unescaping backend binds `p";x`. An unterminated
   `name="open;tail` keeps the leading quote.
2. `filename*=` (F-S3-4): an upload that carries only the RFC 5987 extended parameter
   (`filename*=utf-8''a.txt`) produces no `FILES` entry; the part is published as `ARGS`.
3. Duplicate header (F-S3-6): a part with `Content-Disposition: form-data; name="a"; filename="a.txt"`
   followed by `Content-Disposition: form-data; name="b"` is published as `ARGS:b` with no `FILES` entry; a
   first-wins backend treats it as a file upload named `a`.

## Reference behavior
ModSecurity/Coraza and Go `mime.ParseMediaType` unescape `\"` inside quoted strings; Go and ASP.NET decode
`filename*=`. RFC 7578 forbids more than one `Content-Disposition` per part, and parsers diverge on which
one wins. Confidence: medium (S3).

## Suggested fix
- Unescape `\x` to `x` inside quoted parameter values; normalize an unterminated quote the way common
  backends do.
- Recognize `filename*=` (decode `charset'lang'value`, percent-decoding the value) as a filename source.
- Reject a part with more than one `Content-Disposition` header as `Malformed` (fail closed): disagreement
  between parsers is the attack signal.
Each change alters the pinned fixtures above; update them in the same commit.
Proposed regression tests in `edgion-waf/tests/unit/seclang/body_test.rs`:
`disposition_backslash_escape_matches_rfc_unescaping`, `extended_filename_parameter_produces_files_entry`,
`duplicate_content_disposition_is_malformed`.

## Notes
- Review input: findings F-S3-4 and F-S3-6 (2026-11-07 second-pass body-parser lens).
