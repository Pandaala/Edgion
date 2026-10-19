# Lazy HMAC missing-header diagnostics

Status: deferred for user decision. No runtime change.
Source baseline: `b6520a395`.

## Evidence and scope

HmacVerifier::verify_headers_with formats a String for
MissingRequiredHeader when the first configured enforced header is absent from
the signed-header list. PreparedEnforcedHeader already retains canonical and
original configured spellings. This check follows credential lookup and timestamp
validation and precedes signature verification and any request-body capture.

Standalone HmacAuth consumes the message in its 401 rejection path, preserving
the configured spelling after the fixed prefix. The shared condition evaluator
does not consume the text: synchronous evaluation checks only Verified, and
asynchronous body-condition evaluation discards NoMatch payloads before applying
negation. Those condition failures therefore allocate a diagnostic they never use.

Verifiers are already prepared once per immutable runtime generation. This is
not a proposal to rebuild them or another report of the resolved credential-map
cloning problem. Signed-header membership, keyed MAC preparation and signing-input
construction remain separate hmac-01/02/03 decisions. Successful username ownership
and other failure variants are outside this proposal.

## Decision requested

Choose the representation after assessing maintenance and retained-memory costs:

- Return structured borrowed failure data and format only at the standalone
  rejection sink. Tie a header loan to its verifier/configuration generation,
  not the session; do not unnecessarily retain a mutable-session borrow across
  asynchronous body capture. Review outcome lifetimes and Debug behavior.
- Return a typed index into the prepared enforced-header list. Keep it paired
  with its originating verifier generation and make invalid/cross-generation
  indexing impossible; do not hide a new panic or fallback behind an integer.
- Prepare diagnostic text during construction. This avoids repeated formatting
  but retains extra text for every enforced header, including never-failing
  instances; owned outcome transfer still requires a borrowing/shared-owner design.
- Keep current formatting if the failure-path savings do not justify the added
  ownership machinery. No frequency or throughput measurement has established value.

Do not introduce separate verifier implementations or a caller-specific
verification policy solely to suppress text. The shared verdict must stay identical.

## Acceptance before implementation

Use IMPL with an exact file allow-list and independent reviews. Preserve first
missing header selection in configured order, duplicate configured entries,
canonical membership and original error spelling. Keep credential/timestamp/
signature failure priority, no body IO before header authentication, body-limit
failure distinctions, conditional negate and unresolved behavior, and standalone
authentication delay, fixed log code, status, challenge and response body.
Anonymous and successful metadata paths must remain unchanged.

Exercise actual standalone rejection and both synchronous and asynchronous
condition consumers. Cover multiple missing headers, mixed case, duplicates,
missing signed date, bad timestamp, invalid signature and body-capture suppression.
Preserve existing debug redaction and add no credential names or secret data to
logs. Verify old/new verifier generations remain independent across replacement.

Measure only diagnostic formatting/storage separately from header parsing,
membership construction, signature verification, username ownership and final
response rendering. Removing this String does not make HMAC evaluation or chunk
conditions allocation-free. Compare retained preparation costs if considering
preformatted messages. No full-suite or benchmark result is claimed here.

## Sources

- Gateway `plugins/http/hmac_auth/plugin.rs`: PreparedEnforcedHeader,
  HmacVerifyFailure, verify_headers_with, verify, run_request and reject.
- Gateway `plugins/runtime/conditions/evaluator.rs`: evaluate_sync/evaluate_async.
- Gateway HMAC unit test
  `enforce_headers_are_prepared_once_in_order_with_configured_error_spelling`.
- `skills/04-review/cpu-memory/regression-guards/header-hmac-condition-verifier-prebuilt.md`.
- `skills/04-review/hmac-auth/regression-guards/request-body-hash-after-cheap-auth.md`.
- `skills/04-review/hmac-auth/regression-guards/wire-format-is-edgion-contract.md`.
