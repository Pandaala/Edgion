# HMAC expected body-digest snapshot

Status: deferred for user decision. No runtime change.
Source baseline: `e50477cef`.

## Evidence and scope

After successful header authentication, HmacVerifier::verify calls
HmacAuth::expected_body_digest. The helper parses the Digest header and copies
its trimmed expected text into a String. That owner survives asynchronous body
capture, the logical body-size check and request_body_sha256. A direct borrowed
header view cannot simply replace it while the same session is mutably borrowed
for those operations.

body_digest_matches encodes the computed 32-byte SHA-256 hash into 44 bytes using
padded STANDARD Base64 and compares the text exactly. The expected String can
hold any length accepted by the header parser, including empty and wrong-length
values. This is a separate snapshot allocation, not a body copy or an unbounded
leak claim; transport header limits remain outside this narrow investigation.
The existing stack-encoding optimization removed the computed encoded String,
not this expected-text owner.

## Decision requested

Consider a bounded owned snapshot of exactly 44 raw expected bytes, with a
known-mismatch representation for other lengths, or retain the existing String.
Do not choose the representation without assessing async-future storage, value
copying, API ownership and complexity alongside the saved allocation. No size,
RSS, frequency or throughput measurement establishes the preferred tradeoff.
This decision is separate from signed-header membership, keyed MAC preparation,
incremental signing inputs and lazy missing-header diagnostics (hmac-01 through hmac-04).

A known mismatch must not return early or skip hashing. Current code rejects
missing/malformed/unsupported Digest syntax before capture, but syntactically
accepted wrong-length text still reaches capture, logical-limit checking and
hashing. Their failures take precedence over Body digest mismatch. Moving a
length rejection earlier changes externally visible status and condition flow.
Do not decode Base64 into a more permissive binary equivalence: canonical pad
bits, padding and exact interior bytes are part of current comparison behavior.

## Acceptance before implementation

Use IMPL with an exact allow-list and independent plan/final reviews. Preserve
header authentication before body IO, syntax rejection before capture, logical
limits before hashing, current immutable body-version hashing, cancellation and
capture/read failure mapping. Keep the canonical request BodyStorage; add no body
cache, materialization, cursor, log, metric, schema or policy change.

Test canonical text and equal-length mismatches, empty/truncated/extra text,
noncanonical pad bits, missing padding, outer trimming and interior whitespace.
Exercise correctly signed but wrong-length Digest text combined with capture
failure, logical overflow and hashing failure; assert those outcomes still win
and required body calls still occur. Keep missing/malformed/unsupported syntax
before capture. Verify standalone response/status/error recording and compiled
body-condition failure/negate semantics; synchronous header conditions remain
unchanged. Existing tests do not by themselves prove this new combination matrix.

If prototyped, measure snapshot allocations separately from parser/signing/body
work and inspect actual future/layout effects rather than claiming stack-only
storage inside an async function. The snapshot must own its bytes across session
mutation and must not retain a loan or new source-generation owner.

## Verification and sources

Fresh unchanged-runtime verification passed 57 Gateway HMAC-filtered and 70
condition-system tests, with three shared identities (124 unique tests), plus
workspace all-target check/Clippy with existing warnings. These baseline tests
establish current behavior only; no prototype, benchmark, full suite or live
transport test ran.

- Gateway plugins/http/hmac_auth/plugin.rs: HmacVerifier::verify,
  expected_body_digest, parse_body_digest_header, verify_body_digest_expected,
  body_digest_matches and run_request.
- Gateway plugins/runtime/conditions/evaluator.rs: evaluate_sync/evaluate_async.
- Gateway tests/unit/plugins/http/hmac_auth/plugin_test.rs: signed-body cases,
  logical/physical limits and exact canonical comparison regression.
- skills/04-review/hmac-auth/regression-guards/request-body-hash-after-cheap-auth.md.
- skills/04-review/hmac-auth/regression-guards/wire-format-is-edgion-contract.md.
- skills/04-review/cpu-memory/regression-guards/header-hmac-condition-verifier-prebuilt.md.
