# Decide translated Retry-After value storage

Status: deferred ownership and performance decision for user review.
Source baseline: `62fbafb9b`. No runtime implementation selected.

## Evidence

`AnthropicResponseProcessor::normalize_header` validates the first Retry-After
only for an original 429, copies its text into a String, clears provider headers,
and reconstructs the accepted header. The owned snapshot isolates its lifetime
from the upstream header backing. Other provider headers remain excluded by the
translated-response empty-base contract.

Pingora remove_header can return the first owned HeaderValue while removing all
duplicates and case metadata. A prototype moved that value after validation and
reset its sensitivity to false to match the previous String reconstruction.
Wire and metadata equivalence passed, and a dynamic-header pointer check proved
that both unary and streaming normalization retained the original value bytes.

However, the actual HTTP/1 receive path has different ownership from an isolated
HeaderValue::from_bytes fixture. At pinned fork revision
`1aaa45cc70430bc782990bc20ddee3fef36d6b99`,
`pingora-core/src/protocols/http/v1/client.rs` freezes response_header_read_buf,
then builds each header from KVRef::get_value_bytes(&buf).
`pingora-core/src/utils/mod.rs` obtains a Bytes slice; ordinary field values pass
through `pingora-http/src/lib.rs::header_value_from_raw` without copying into a
smaller owner. Moving a tiny retained header may therefore retain the whole
receive allocation rather than just its visible bytes.

The client session also retains buf. Extra lifetime depends on other session,
response and downstream owners; this source evidence is not a measured memory
regression or proof that every transfer retains memory longer. It does invalidate
an unconditional claim that removing the text copy is a net resource improvement.
The prototype and its tests were withdrawn; production retains the copy.

## Decision alternatives

- Keep the current isolated text snapshot if small header copies are preferable
  to longer shared-buffer retention.
- Transfer the accepted HeaderValue only after proving the actual lifetime and
  memory tradeoff is acceptable across relevant transports and response paths.

Neither alternative changes the allowed header contract. Do not add a cache,
new parser, speculative copy-size threshold, unsafe backing inspection or new
request-level telemetry to conceal the ownership decision. This task is distinct
from pending response-header ownership and fixed-name/header preparation tasks.

## Acceptance for a selected implementation

- Freeze an exact IMPL allow-list and obtain independent plan and final reviews.
  Inspect production HTTP/1 receive backing, session ownership, response hooks,
  downstream writes, failures and cancellation. Assess other supported transport
  representations separately; do not generalize HTTP/1 evidence to HTTP/2.
- Measure allocation calls, copied bytes, backing capacity and release time with
  a short accepted header in a large real shared receive buffer. Include unary
  and streaming conversion, slow downstreams, completion/error/cancellation and
  independently owned/static values. A pointer assertion alone proves reuse,
  not reduced peak memory, retained memory or throughput improvement.
- Preserve the original-status 429 gate, first-value policy, UTF-8 validation,
  unary untrimmed 64-byte limit, date/digit validation, exact retained whitespace,
  duplicate removal, normalized header order/case and false sensitivity metadata
  from the existing String reconstruction. Invalid first values must not fall
  through to a valid duplicate. Preserve status, usage markers and body modes.
- Keep the empty-base provider-header boundary: only synthesized Content-Type,
  Cache-Control and accepted Retry-After survive this hook. Keep redaction,
  canonical access-log ownership and observability restraint unchanged.

## Investigation and verification

The withdrawn prototype first passed 33 converter tests with a frozen old 429
normalization oracle covering both modes, sensitivity flags, valid/invalid,
opaque/empty, date/whitespace and 64/65-byte first values plus duplicates. After
storage transfer and a pointer assertion, 34 converter, 39 dispatch and 14
canonical SSE tests passed (87 total). These results establish the limited
prototype properties above; they do not validate shared-buffer lifetime benefit.
The prototype also passed formatting, workspace check/Clippy and seven repository
guards before being withdrawn. Final unchanged-source results are recorded in
INDEX.md separately; no prototype source or new test is part of this task.

Original knowledge:
`skills/04-review/security/regression-guards/anthropic-translated-responses-rebuild-headers.md`.
No full suite, live-provider test or memory/throughput benchmark was run.
