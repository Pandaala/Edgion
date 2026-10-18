# JWT claim condition evaluation ownership

Status: deferred design decision; existing per-chunk allocation tension remains
unresolved. Source baseline: `532e61884`.

## Evidence

`plugins/runtime/conditions/extract.rs::extract_jwt_claim_values` obtains an owned
context string, parses a fresh serde_json Value, resolves the claim, then eagerly
creates Vec<String> candidates. Strings are cloned; numbers and booleans are
formatted. Arrays contribute scalar elements only. Objects/null/nested non-scalars
produce no candidates. The source is jwt_claims when present; oidc_claims is read
only when jwt_claims is absent, not when it is malformed or lacks the selected key.

`conditions/evaluator.rs::evaluate_jwt_claim_match` passes that completed vector
to evaluate_multi_match, whose any-match walk can stop early. The response-body
wrapper in `plugins/runtime/conditional_filter.rs` evaluates the compiled conditions
again for each chunk. Thus even a first-candidate match has already paid extraction
and candidate construction costs. Condition descriptors, regex initialization,
time bounds and dependency bindings are already cold-compiled; they are not the
per-evaluation JSON parsing problem.

Context strings are live plugin state. The production adapter clones get_ctx_var
values; ctx.rs set_ctx_var/remove_ctx_var can change them between evaluations.
The canonical claim resolver splits on dots and performs string-key Value::get
for every segment; it does not interpret numeric segments as array indexes or
prefer an exact literal dotted key.

The design rules and response-body trait prohibit per-callback allocation.
The current JWT path still allocates. This is an unresolved existing cost/rule
tension, not an accepted exception or proof of compliance. Deferring a design
choice does not waive that constraint. Historical fixture measurements in review
knowledge are not fresh verification or workload performance estimates.

## Decision alternatives

- A per-evaluation parsed owner with lazy borrowed scalar candidates can avoid
  String clones for string values and avoid the complete candidate vector. Only
  visited numeric/boolean candidates would need formatting. This requires a clear
  owner/iterator or callback API; it retains context copying and JSON parsing and
  therefore does not by itself resolve the per-chunk allocation requirement.
- Reusing parsed request-owned claims can remove repeated parsing, but every write,
  removal and precedence change must invalidate or replace the right derived state.
  Account for malformed JSON, source absence, retries and memory retention. Keep
  one canonical owner; do not attach independent caches to every predicate.
- Retain current behavior while gathering evidence and designing the boundary,
  with the allocation-rule tension explicitly open. This is a sequencing option,
  not acceptance of a permanently allocating body predicate.

Do not cache a boolean predicate result or freeze claims at the first chunk.
Do not remove opt-in claims serialization from JWT/OIDC: writing JSON to context
is a separate requested feature, and its existing interface serves other readers.
A future implementation needs a separately reviewed IMPL scope, including any
trait, context owner, producer, mutation or knowledge changes.

## Acceptance

Preserve jwt/oidc precedence, malformed/missing behavior, dot-key lookup, scalar
formatting/order, array filtering, any-match short-circuit, negation and uncompiled
regex diagnostics (empty candidates must not acquire a diagnostic). Exercise
context replacement/removal between chunks, absent-to-present changes, jwt removal
revealing oidc, multiple conditions and producer ordering. Keep accessor validation
in shared Condition validation. Retain claims log redaction and the blocked context
key inventory; add no claims, token or per-chunk diagnostic output.

Measure allocations, retained memory and repeated-evaluation CPU with small/large
claims and first/late/missing matches. Report separately which source copy, parse,
candidate and formatting costs remain; do not present an allocation reduction as
full satisfaction of the body-callback constraint or as measured throughput.

Original knowledge: `skills/04-review/cpu-memory/regression-guards/response-condition-live-input-borrowing.md`,
`skills/04-review/cpu-memory/review-evidence/oidc-jwt-claims-json-serialization-opt-in.md`,
`skills/04-review/plugin-metadata/mandatory-design/jwt-claim-accessor-validation-belongs-to-condition.md`,
and `skills/04-review/ctx-set/regression-guards/claims-access-log-filter.md`.
This task records current source inspection only; no runtime change or fresh
benchmark was performed.
