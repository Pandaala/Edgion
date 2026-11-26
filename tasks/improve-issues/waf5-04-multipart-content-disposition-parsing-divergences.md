# waf5-04: Decide multipart quoted-string and extended filename handling

- **Category**: correctness
- **Severity**: Low
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/body.rs:767-813` (`disposition_parameters`: strips only a matched pair of outer `"`, keeps backslash escapes and an unmatched leading quote; recognizes `name` and `filename` only, `:792-796`)
  - `edgion-waf/tests/unit/seclang/body_test.rs::multipart_disposition_accepts_consistent_values_and_tolerant_grammar`

## Summary
Two remaining parameter-parsing differentials can make Edgion and the application bind a multipart part to
different names. A rule or CRS runtime exclusion scoped to `ARGS:<name>` (`ctl:ruleRemoveTargetById ...
ARGS:name`) then applies on one side only, and a file part can be seen as an ordinary argument or not at all.

## Current behavior (evidence)
1. Quoted-string escapes (F-S3-4): `Content-Disposition: form-data; name="p\";x"` yields the Edgion argument
   name `p\";x` (backslash retained); an RFC quoted-string unescaping backend binds `p";x`. An unterminated
   `name="open;tail` keeps the leading quote.
2. `filename*=` (F-S3-4): an upload that carries only the RFC 5987 extended parameter
   (`filename*=utf-8''a.txt`) produces no `FILES` entry; the part is published as `ARGS`.

Duplicate Content-Disposition headers and conflicting name or filename values now return
a typed multipart parse failure. See the [parser outcome contract](../../skills/04-review/waf/mandatory-design/request-body-parser-outcome.md).

## Reference behavior
ModSecurity/Coraza and Go `mime.ParseMediaType` unescape `\"` inside quoted strings; Go and ASP.NET decode
`filename*=`. RFC 7578 forbids more than one `Content-Disposition` per part, and parsers diverge on which
one wins. Confidence: medium (S3).

## Suggested fix
- Unescape `\x` to `x` inside quoted parameter values; normalize an unterminated quote the way common
  backends do.
- Decide how to inspect extended filenames from deployed clients. [RFC 7578 section 4.2](https://www.rfc-editor.org/rfc/rfc7578#section-4.2)
  prohibits senders from using this parameter. Specify the receiver's behavior for extended-only and mixed filenames.
Each change alters the pinned fixtures above; update them in the same commit.
Proposed regression tests in `edgion-waf/tests/unit/seclang/body_test.rs`:
`disposition_backslash_escape_matches_rfc_unescaping` and a case covering the selected extended filename policy.

## Notes
- Review input: findings F-S3-4 and F-S3-6 (2026-11-07 second-pass body-parser lens).
