# Bounded Redis GET text validation ownership

Status: deferred for user decision. No runtime change.
Source baseline: 3bbb7da79.

## Evidence and scope

`bounded_get_result` in `edgion-gateway/src/link_sys/providers/redis/ops.rs`
checks the raw reply kind and original byte length, validates UTF-8, then returns
`BoundedGet::Value(Bytes)`. The only production consumer is `RedisGetBounded` in
`plugins/http/dsl/lang/builtins.rs`. It validates those bytes again, charges
`ValueWorkBudget` for twice the text byte length, then allocates the independent
`Arc<str>` used by the DSL result.

Successful text therefore crosses two UTF-8 checks. These are scans, not two
payload copies. Returning ordinary Bytes does not express the earlier text
validation in its type. The consumer's Internal error is a defensive invariant
failure; ordinary invalid UTF-8 is already rejected by the provider and becomes
the fixed `invalid_response` result map.

The caller requires a positive requested bound and caps it by `maxStringLen` and
the DSL JSON limit of 65536 bytes. Provider length admission precedes UTF-8, so
oversized invalid bytes report TooLarge rather than InvalidResponse. Null,
non-string replies, typed timeout and server-error frames retain their separate
handling. Server-error details are not copied into script-visible output. Fred
has already aggregated the complete reply; none of these checks bounds network
frame allocation.

## Decision requested

Retain the duplicate validation, or review a canonical safe text carrier that
preserves the admitted Bytes owner and exposes validated text without rescanning.
Reuse a suitable existing representation if available; dependency selection,
constructor visibility and provider API ownership need review. Do not create a
new unchecked UTF-8 conversion just to remove a scan.

Alternatively, consider moving validation to the sole consumer. That changes the
provider contract and error classification boundary, so it requires an explicit
design even if script-visible results can be preserved. Do not simply delete a
check and let invalid input reach the current Internal-error branch.

Returning a String or Arc directly from the provider is not a drop-in solution:
it moves owned text allocation before the VM work charge. Keep validation and
work admission before result allocation. Preserve immutable backing lifetime,
and measure retained Bytes backing as well as copied output. No new cache,
transport buffer, response aggregation or relaxed budget is proposed.

This is distinct from Redis 05's script-array conversion and DSL 01's input
string ownership. No scan-time or request-throughput improvement is measured.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Compare CPU work and allocations for
empty, ASCII, multibyte, invalid and exact-cap text, including rejected work
budgets. Prove successful text cannot be constructed without validation under
any permitted carrier constructor. Preserve byte-based limits and exact text.

Verify oversize-before-invalid precedence, Null, unsupported reply kinds,
SimpleString/BlobString, server errors, typed timeout and dependency errors.
Distinguish provider zero-cap behavior from the caller's positive-bound admission;
include a clamped effective cap and late work-budget exhaustion. Preserve fixed
error classes, live-client resolution, command deadlines, cancellation cleanup
and the post-admission Arc copy. A retained small slice of a large backing is a
memory consideration, not evidence that Fred's aggregation is now bounded.

At this baseline, all 15 Redis operation unit tests and 501 Gateway DSL tests
passed. Existing bounded-GET tests cover multibyte byte limits, oversize invalid
bytes, in-bound invalid UTF-8, Null/non-string responses and private server-error
classification. They do not establish a new carrier's allocation/lifetime behavior,
live Redis transport behavior or a performance gain.

Original contracts: `skills/01-architecture/02-gateway/dsl-engine-security-model.md`
and `skills/04-review/dsl/regression-guards/bounded-json-reconstruction.md`.
