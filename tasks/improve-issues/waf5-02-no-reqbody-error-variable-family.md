# waf5-02: No `REQBODY_ERROR` / `MULTIPART_STRICT_ERROR` variable family, so body-parse anomalies cannot be scored by rules

- **Category**: design-error
- **Severity**: Medium
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/semantic/mod.rs:192-204` (`registry_enum!(CollectionKind ...)`: no `REQBODY_ERROR`, `REQBODY_ERROR_MSG`, `REQBODY_PROCESSOR_ERROR`, `MULTIPART_STRICT_ERROR`, `MULTIPART_UNMATCHED_BOUNDARY`, `MULTIPART_DATA_BEFORE`, `MULTIPART_HEADER_FOLDING`, `URLENCODED_ERROR`)
  - `edgion-waf/src/seclang/body.rs:835-870` (`percent_decode`: invalid `%zz` is kept literally, comment at `:859-860`, no error recorded)
  - `edgion-waf/src/seclang/body.rs:660-664` (multipart header folding returns `Malformed`, which only fails closed or falls back to raw)
  - `edgion-waf/src/seclang/transaction/phases.rs:34` (`request_body_error()` is a host API, not a rule-visible variable)

## Summary
Edgion has two outcomes for a body anomaly: tolerate it silently (urlencoded invalid escapes) or treat it as
`Malformed` (fail closed in `On`, raw fallback in DetectionOnly, waf5-01). There is no intermediate
error flag that a rule can test. The ModSecurity/CRS 920/922 "request body / multipart strict error" rule
family keys on exactly those variables, so it cannot be ported, and an unknown variable name is rejected at
compile time.

## Current behavior (evidence)
- PoC (S3): `Content-Type: application/x-www-form-urlencoded`, body `a=%zz&b=attackval`. Edgion publishes
  `ARGS:a = %zz` (literal) and `ARGS:b = attackval`; no error variable exists. Value content is still
  inspected, so this is a scoring/visibility gap, not a content bypass by itself.
- `tests/unit/seclang/body_test.rs::auto_and_urlencoded_preserve_duplicate_order_and_decode_tolerantly`
  pins the tolerant decode.
- The shipped CRS profile `config/waf/crs-4.25.1-supported-v1` contains no rule 200000-200005 (the
  ModSecurity recommended-config body-error rules), so nothing currently depends on these variables.

## Reference behavior
ModSecurity v3 sets `REQBODY_ERROR` (and `URLENCODED_ERROR`) on invalid percent sequences and populates the
`MULTIPART_*` flags; CRS and the recommended configuration (200002/200003) deny on them. Confidence: high (S3).

## Suggested fix
Design decision for the owner: add a bounded, read-only error-variable family populated from the existing
`BodyError` classification (and from tolerated anomalies such as invalid escapes), wired to
`request_body_error`, so operators can port the 920/922 strict-error rules. Pairs with waf5-01.
Proposed regression test: `urlencoded_invalid_escape_sets_reqbody_error` in
`edgion-waf/tests/unit/seclang/body_test.rs` once the variable exists.

## Notes
- Review input: finding F-S3-2 (2026-11-07 second-pass body-parser lens).
