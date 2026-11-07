# waf5-03: Grammatically valid JSON numbers outside the f64 range are rejected as malformed JSON

- **Category**: correctness
- **Severity**: Low-Medium (over-block in `On`, structured-view loss in DetectionOnly)
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/json_number.rs:29-61` (`from_validated_lexeme`: `raw.parse::<f64>()`, then `if !value.is_finite() { return Err(()) }` at `:55-57`)
  - `edgion-waf/src/seclang/json_stream.rs:998-1019` (`finish_number`: `Err` mapped to `JsonStreamError::Number` at `:1011`)
  - `edgion-waf/src/seclang/json_inspection_runtime.rs:100-111` (`malformed()` includes `Number`) and `:228-247` (fail closed in `On`, raw fallback in DetectionOnly)

## Summary
The JSON grammar FSM accepts `1e999` as a valid number lexeme (RFC 8259 places no range limit on numbers).
The value conversion then sees `f64::INFINITY` and returns an error, which is classified as a malformed
document, although several common backend parsers accept the document.

## Current behavior (evidence)
- PoC (S3): `Content-Type: application/json`, body `{"user":"attackval","n":1e999}`.
  - `On`: the request is denied as malformed JSON although it is valid JSON (false positive).
  - DetectionOnly: `json.user` and `json.n` are not published; only the raw body is inspected (waf5-01),
    while the backend parses the object and acts on `user`.
- The selective path (`self.selective.is_some()`, `json_stream.rs:1007-1010`) already forwards the exact
  spelling without f64 coercion; only the non-selective path converts.

## Reference behavior
The lexeme is valid RFC 8259 JSON (confidence: high). Parsers disagree on the value: Python `json`
(`inf`) and JavaScript `JSON.parse` (`Infinity`) accept the document; Jackson can read it as `BigDecimal`;
Go `encoding/json` and `serde_json` report a per-value range error when decoding into `float64`/`f64`, but
both accept it as `json.Number` / with arbitrary precision. yajl in ModSecurity v3 parses the document and
reports the argument. Confidence on individual library details: medium. The differential therefore matters
for Python/JavaScript/JVM backends.

## Suggested fix
The FSM already guarantees a valid lexeme, so inspection should proceed: on a non-finite f64, keep the raw
spelling and a saturated (or "non-finite") numeric representation instead of returning `Err`. Keep the
"do not deserialize the whole document" contract.
Proposed regression test: `overflowing_but_grammatical_number_is_accepted_not_number_error` in
`edgion-waf/tests/unit/seclang/json_stream_test.rs` - feed `{"n":1e999}`, assert a Number token, not
`JsonStreamError::Number`.

## Notes
- Review input: finding F-S3-3 (2026-11-07 second-pass body-parser lens). Adjacent to waf3-01 (numeric
  operators) but independent: this one rejects the document before any operator runs.
