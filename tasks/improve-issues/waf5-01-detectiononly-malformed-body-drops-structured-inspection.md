# waf5-01: In DetectionOnly a malformed multipart/JSON/XML body falls back to raw-only publication and drops every structured view

- **Category**: design-error
- **Severity**: Medium (scoped to DetectionOnly / monitoring deployments)
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/transaction/phases.rs:92-113` (`publish_non_json_body`: on `BodyError::Malformed` / `MultipartBoundary` in DetectionOnly, call `build_fallback_publication*` and keep the error in `body_error`)
  - `edgion-waf/src/seclang/body.rs:193-233` (`build_publication_inner`: with `parse_derived_views == false` only `REQUEST_BODY`, `REQUEST_BODY_LENGTH`, `REQBODY_PROCESSOR` and `ARGS_COMBINED_SIZE` are pushed; the processor `match` at `:221` is skipped)
  - `edgion-waf/src/seclang/json_inspection_runtime.rs:228-247` (JSON: `malformed(error)` in DetectionOnly abandons the collector and publishes the raw prelude)
  - `edgion-waf/src/seclang/body.rs:612-617` (CRLF required after a delimiter) and `:649` (`\r\n\r\n` header separator required) - the strictness that turns an LF-only multipart body into `Malformed`

## Summary
When a request body is malformed for its selected processor, `On` mode fails closed, which is correct.
`DetectionOnly` instead publishes a fallback view that contains only the raw body and three scalar
variables. No `ARGS`, `ARGS_NAMES`, `FILES`, `FILES_NAMES`, `MULTIPART_PART_HEADERS`, JSON or XML candidate
is produced. The only trace is `Transaction::request_body_error()` (`phases.rs:34`), which rules cannot read
(see waf5-02). A monitoring deployment therefore sees nothing structured for exactly the bodies an
attacker shapes to be "almost valid".

## Current behavior (evidence)
- PoC (review lens S3, 2026-11-07): `Content-Type: multipart/form-data; boundary=x`, body with bare-LF line
  endings: `--x\n` + `Content-Disposition: form-data; name="q"\n\n` + `attackval\n--x--\n`. The parser
  requires CRLF after the delimiter and a CRLF CRLF header separator, so it returns `Malformed`. In
  DetectionOnly `ARGS:q` is never published; only `REQUEST_BODY` rules can see `attackval`. Go
  `mime/multipart` accepts bare LF and the backend binds `q=attackval`.
- The same fallback applies to any JSON lexical error (`Syntax | Utf8 | Escape | Number | Incomplete`) and to
  XML `Syntax | Character | Incomplete`; see waf5-03 for a JSON case where the document is actually valid.

## Reference behavior
ModSecurity v3 keeps the parseable parts in `ARGS`/`FILES` and sets `REQBODY_ERROR` /
`MULTIPART_STRICT_ERROR`; CRS 920/922 rules score the error. Coraza is similar. Confidence: high (S3).

## Suggested fix
Contract decision for the owner, paired with waf5-02:
1. Expose a `REQBODY_ERROR`-equivalent variable (waf5-02) so CRS-style strict-error rules work in both modes.
2. In DetectionOnly, attempt bounded best-effort per-part extraction before falling back to raw, or
3. at minimum document in the WAF user guide and the review knowledge that DetectionOnly drops all
   structured inspection on any malformation.

Proposed regression test: `detectiononly_malformed_multipart_documents_structured_view_loss` in
`edgion-waf/tests/unit/seclang/transaction/mod_test.rs` - publish the LF-only body in DetectionOnly, assert
`request_body_error()` is `Some(Malformed { processor: Multipart })` and pin whether `ARGS:q` exists.

## Notes
- Review input: finding F-S3-1 (2026-11-07 second-pass body-parser lens). Related: waf5-02, waf5-03, waf-05.
