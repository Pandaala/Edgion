# Query and Cookie stripping survivor storage

Status: deferred representation and CPU/allocation decision for the user.
Source baseline: `ff0551111`.

## Evidence

`edgion-gateway/src/plugins/http/common/auth_common.rs` retains borrowed survivors
in Vec<&str> before joining them into an owned output. `strip_query_param` collects
nonempty unmatched pairs even when no credential is ultimately removed; that
no-match path drops the vector without writing a replacement. Plain parameter
names already avoid decoding storage, while names with percent/plus markers use
the canonical decoder. The input query itself is borrowed from the live URI.

`plan_cookie_strip` consumes an arbitrary iterator of borrowed Cookie lines once,
trims and classifies pairs, then joins survivors with semicolon-space separators.
It distinguishes no valid UTF-8 line from present lines with no surviving pairs.
Unlike query stripping, Cookie stripping merges/replaces survivors even when no
name matched or the names list is empty. KeyAuth and JwtAuth share both helpers;
OIDC also strips its session and state cookies through the shared Cookie helper.

The vectors retain slices, not copies of every payload. Pointer-container storage
scales with survivor count, and the joined output scales with surviving bytes.
Current unit evidence establishes exact output capacity for the selected Cookie
fixtures. The subsequent owned HeaderValue conversion preserves the output buffer
and allocates zero times in those fixtures. Pinned http 1.5.0 transfers the String
through bytes 1.12.1; its From<Vec<u8>> implementation uses a boxed-slice path when
length equals capacity, but allocates Shared metadata and retains capacity when
they differ. This is relevant to an alternative output builder, not evidence of
whole-callback zero allocation or measured throughput.

## Decision alternatives

- Keep borrowed survivor vectors and exact joining until measurements justify
  changing this representation.
- Materialize a query replacement only after finding the first removable pair.
  Evaluate the no-match benefit independently of the Cookie path. Preserve empty
  pair filtering on matched queries without rewriting unmatched queries, and
  avoid unnecessarily decoding the already inspected prefix again.
- Append survivors directly to a String. Compare growth reallocations, copies and
  final retained capacity against the removed pointer vector. Reserving an input
  upper bound may avoid growth but leave excess capacity when credentials are
  stripped. Cookie bounds must include inserted separators across header lines:
  summing line lengths alone is insufficient for separate valueless pairs.
- Count output bytes first and fill an exactly sized output in another pass.
  Account for repeated parsing, key decoding and membership checks. The current
  Cookie helper accepts an Iterator without a Clone/rewind requirement; a second
  pass needs an explicitly reviewed source interface or retained decisions, not
  an assumption that every caller's iterator can restart.
- Use bounded inline survivor storage with a heap fallback if measurements justify
  the stack footprint and threshold. Do not impose a new input limit to make the
  implementation convenient, or replace borrowed slices with owned pair copies.

Shrinking a buffer is not a free correction: it may reallocate/copy, and must be
included in cost measurements. Excess capacity refers to retained allocation
size, not a claim that the new output contains removed credential bytes. Do not
introduce unsafe views, extend borrows across request mutation, or build a global
scratch pool. Query and Cookie matching have different semantics; sharing an
output-storage technique must not unify their matching rules accidentally.

## Acceptance for a selected implementation

Preserve the live-query/frozen-extraction boundary, percent-decoded query names,
plain-name fast path, malformed-token behavior, duplicate removal and exact raw
surviving query bytes. Unmatched query syntax remains byte-identical with no
setter call; matched queries retain their established empty-pair removal.

For Cookies, inspect every line, retain the existing UTF-8 filtering, whitespace
trimming, raw name matching and first-equals split, and remove every selected
name in one logical operation. Preserve Absent/RemoveHeader/Replace distinctions,
including empty lines, no survivors, no matches and an empty names list. Continue
merging multiple lines into one replacement rather than dropping unrelated fields.

End immutable request borrows before mutation. Preserve header validation,
sensitivity marking, protected-header handling, deferred cleanup and ignored
setter errors. Query updates must leave path forwarding and HTTP/2 authority to
their existing owners. Keep the existing Cookie owned-conversion allocation and
pointer regression, or independently review a measured whole-path tradeoff before
changing that performance contract; do not silently lose it while deleting a Vec.

Compare matched/unmatched queries, absent/empty/multiple Cookie lines, few/many
short pairs, long values, high removal ratios and percent-heavy names. Measure
allocation calls, requested and retained bytes, stack footprint and CPU separately,
including output conversion and mutation as well as planning. Fixtures belong
outside measured closures; no production observability or secret-bearing logs
are needed. A selected design requires its own IMPL plan, review and verification.

## Original knowledge and baseline

- `skills/04-review/key-auth/regression-guards/strip-query-param-no-match-preserves-query.md`
- `skills/04-review/key-auth/regression-guards/hide-credentials-query-cookie-strip.md`
- `skills/04-review/jwt-auth/regression-guards/hide-credentials-query-cookie-strip.md`
- `skills/04-review/uri-normalization/mandatory-design/normalization-contract.md`

Fresh baseline verification passed 32 shared-auth, 14 KeyAuth, 47 JwtAuth and four
OIDC cookie-strip tests (97 total), formatting check, workspace check and Clippy
with existing unrelated warnings. These validate the current implementation;
no alternative representation or throughput result has been measured. This task
makes no runtime change.
