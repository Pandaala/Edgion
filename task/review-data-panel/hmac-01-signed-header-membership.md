# HMAC signed-header membership ownership

Status: deferred decision for the user. Source baseline: `8c3a6c5db`.

## Evidence

`edgion-gateway/src/plugins/http/hmac_auth/plugin.rs::parse_authorization`
normalizes signed-header names into an ordered Vec<String>, then builds a temporary
HashSet<&str> to reject duplicates. That set does not survive the parser return.
`HmacVerifier::verify_headers_with` builds another borrowed set from the returned
vector when enforce_headers is Some, after credential lookup, signed-date selection
and clock validation. This repeats hashing and table construction on that path;
it is source evidence, not a measured allocation total or throughput result.

The vector order is the signing-string order. Duplicate signed headers are rejected
after ASCII lowercase normalization. Configured enforced headers are already
prepared at verifier construction with both canonical and original spelling;
verification reports the first missing configured header in configuration order.
The existing test `enforce_headers_are_prepared_once_in_order_with_configured_error_spelling`
locks that behavior, including duplicate configured entries.

The parser and `HmacMatchConfig::detect_validation_error` in
`edgion-resources/src/resources/edgion_plugins/hmac_match.rs` have no local count
cap for signed tokens or enforced headers. This does not mean transport inputs
are unbounded. Replacing membership with nested scans changes expected membership
work from O(N + R) to O(N * R), excluding string hashing/comparison lengths, where
N is the signed-header count and R is the configured required-header count.
No workload distribution or crossover measurement currently justifies that choice.

## Decision alternatives

- Retain the current representation while measuring header counts, allocation
  traffic and CPU cost in bounded offline fixtures, including long token names.
- Use direct scans for a reviewed bounded-small case and retain hashed membership
  elsewhere. Define the threshold from evidence and preserve behavior; an arbitrary
  new request/configuration rejection limit is not an optimization.
- Retain ordered ownership and membership in one representation. `indexmap` is
  already a Gateway dependency, but an ordered set still has its own table/vector
  memory and API costs. Compare it against the current Vec plus temporary tables
  before choosing it. Do not keep cloned String keys merely to avoid ownership
  work, and do not introduce unsafe self-references from a set into a sibling Vec.

This is a representation and complexity decision, not permission to remove
signed-header duplicate detection, reorder signature components, change required
headers, or weaken authentication. No runtime change is made by this task.

## Acceptance for a selected implementation

Preserve lowercasing, whitespace tokenization, case-insensitive duplicate rejection,
unknown authorization-parameter validation, parsing/error precedence, and raw
current-query signing. Preserve x-date preference over date, credential/date/clock
checks before required-header checks, configured first-missing spelling/order,
constant-time MAC verification and signature-before-body-capture ordering.

Exercise empty/direct-constructor policy cases, duplicate required entries, absent
and duplicate signed names, many/long signed names, both request-target spellings,
and the fixed SHA-256/384/512 signing vectors. Verify standalone HmacAuth and
compiled header/body condition consumers with their distinct failure policies.
Measure both retained representation and transient allocation/CPU work; no new
request logs, secret-bearing diagnostics or production metrics are needed.
A smaller membership cost does not prove allocation-free body-stage conditions.

Relevant original knowledge:
- `skills/04-review/hmac-auth/regression-guards/wire-format-is-edgion-contract.md`
- `skills/04-review/hmac-auth/review-evidence/signed-headers-scope.md`
- `skills/04-review/hmac-auth/regression-guards/request-body-hash-after-cheap-auth.md`
- `skills/04-review/cpu-memory/regression-guards/header-hmac-condition-verifier-prebuilt.md`

This record is based on current source inspection, not fresh runtime tests or a
performance benchmark. A chosen implementation requires its own IMPL review.
