# Evaluate CSRF decoded-token ownership

Status: deferred for user decision. Priority: P3. Source baseline: baf852e4e.
Documentation only; no runtime or token-format change.

## Evidence and decision

`plugins/http/csrf/plugin.rs::run_request` already borrows the selected header
and canonical Cookie value, compares them and calls `CsrfToken::decode` before
verification. In `plugins/http/csrf/token.rs`, decode allocates Base64-decoded
bytes and deserializes `random` and `sign` into owned Strings. That byte buffer
is released when decode returns, before the plugin samples verification time
and computes HMAC. The returned token keeps only its fields, not the complete
JSON document.

A private local borrowed decoding view could avoid copies of unescaped string
contents, but would retain the decoded document through verification. Whitespace
and ignored fields can make that document much larger than the useful strings.
Escaped strings require decoded storage; replacing String with borrowed str
would reject currently accepted escapes. A Cow representation needs explicit
borrowing support and evidence, not an assumption that changing the field type
makes ordinary Deserialize borrow. It may add branches and ownership complexity.

Keep current ownership as a valid option. If measurement justifies investigation,
compare a scoped decode-and-verify owner with the existing owned token. Do not
make the public issuance representation lifetime-dependent merely to share a
verification type. No new representation, parser, public API or implementation
is selected. This differs from the existing
[cookie-header ownership decision](../../skills/04-review/csrf/regression-guards/set-cookie-append.md#csrf-cookie-producer-ownership-csrf-01).

## Required boundaries

Preserve the locked STANDARD Base64 acceptance, serde JSON contract and fixed
TokenDecodeError mapping. Cover object and derived-struct sequence forms, escaped
field names, missing/duplicate fields, unknown fields, integer range/type, invalid
UTF-8, trailing whitespace and trailing documents. Do not replace the parser
with field extraction that accepts malformed tails or changes duplicate handling.

Generation uses a 32-byte random value encoded as hex, but verification accepts
the supplied nonce string as signed data; do not impose a new 64-hex nonce rule.
Preserve exact UTF-8 bytes after JSON decoding, the existing signed timestamp
format, HMAC fragments and exact lowercase signature comparison. The supplied
signature's 64-byte check follows lifetime checks; retain that order and the
full XOR fold for every length-valid candidate. No MAC preinitialization, token
cache, token normalization or new size cap is authorized by this investigation.

Preserve current-method safe-method handling, header/Cookie precedence and exact
equality, canonical cookie parsing, fixed denial messages and token-free logs.
Keep verification clock semantics, pre-epoch failure, checked age arithmetic,
max-age configuration and generation/rotation boundaries. Borrowed fields must
not escape their decoded buffer or remain across unrelated session mutation.
Do not expose tokens in Debug output, diagnostics or measurement fixtures' logs.

## Acceptance after user selection

Build a differential decode/verify matrix against the original implementation
before changing production code. Include ordinary and escaped string values,
empty and long nonce/signature fields, unknown nested data, field-order and
sequence-form cases, malformed Base64/JSON and all existing lifetime/signature
boundaries. Use lint-permitted UTF-8 examples. Existing tests are a baseline,
not proof of these additional cases. Keep issuance bytes and cookie behavior.

Measure complete parsing and verification with fixtures outside the measured
region. Report allocation calls, cumulative requested bytes, peak live bytes,
retained bytes through HMAC and release timing separately. Compare ordinary
issued tokens, escaped inputs and documents dominated by ignored fields or
whitespace. Reduced string copies alone do not establish lower peak memory,
CPU cost or request throughput. Obtain an independent implementation plan and
final review after the user's decision.

Original contracts: [signature fragments](../../skills/04-review/csrf/regression-guards/signature-input-fragments.md)
and [execution-time method and borrowed Cookie boundary](../../skills/04-review/csrf/regression-guards/execution-time-method-rewrite-boundary.md).
Fresh unchanged-code checks passed 20 Gateway CSRF tests and workspace all-target
check/Clippy with existing warnings. No prototype, new parity matrix, benchmark,
full suite or live integration test ran.
