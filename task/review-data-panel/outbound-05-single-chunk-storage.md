# Evaluate bounded outbound single-chunk storage

Status: deferred ownership and performance decision for the user.
Baseline: `3a92db33b`. No runtime change or selected representation.

## Evidence

`util/http_body.rs::stream_body_chunks_capped` starts with an empty Vec and
reserves `min(8192, max_bytes)` on the first successful nonempty chunk. Every
received chunk is copied into that owner, including a response containing only
one nonempty chunk. The returned Bytes owns the collector allocation rather
than retaining the delivered transport chunk. Empty/error/pending initial
responses already avoid accumulation allocation.

Retaining a first Bytes until EOF, with promotion on later nonempty data, could
avoid an intermediate copy for some responses. It also changes ownership while
awaiting the next item and after returning. A small Bytes slice may retain a
large backing allocation; its visible length does not reveal retained capacity
or exclusivity. Current copying releases each delivered chunk after processing,
but Vec growth and delivered chunks still contribute to peak memory. Neither
current nor proposed logical caps prove total heap or RSS bounds.

This helper serves canonical outbound HTTP calls, DSL, OIDC JSON reads, Wasm
callouts, cold Wasm URL/OCI acquisition and Elasticsearch bulk receipts. Caller
lifetimes differ. Wasm callout and OCI acquisition convert collected Bytes into
owned output; input ownership can therefore shift a copy downstream rather than
remove it. Inspect actual downstream conversions and cache-capacity accounting
before claiming an end-to-end benefit. Cold consumers constrain shared helper
changes, but are not permission for broader cold-path cleanup.

## Decision and constraints

Compare current copying with a reviewed first-chunk/promotion representation.
Retaining current behavior is valid. Do not presume transport buffers are small,
unique or independently allocated, or add an unbounded pool, global scratch,
another response cache or a second collector. Keep this mechanism limited to
bounded separate outbound exchanges; it grants no main proxy response aggregation
or response BodyStorage permission.

- Preserve zero-cap behavior, leading/interspersed empty chunks, exact byte
  order, capped-prefix output and the overflow flag. Stop polling immediately
  at the first proven overflow; do not return success merely on reaching the
  exact cap because EOF or a later error still matters.
- Preserve read errors, caller deadlines and cancellation. Verify prompt drop
  of both stream and any retained chunk after receiving nonempty data and then
  becoming pending, not just cancellation before the first chunk.
- Preserve every caller's TooLarge/read-error/status mapping and rejection of
  truncated bodies. This task does not authorize changing the helper API to
  discard its capped prefix even if some callers currently discard it.
- Define backing-allocation ownership, promotion copy count, oversized first
  chunk handling, final output ownership and downstream conversion behavior.
  Do not charge unknown retained capacity as Bytes::len or silently weaken
  payload accounting in the
  [Wasm artifact memory owner](../../skills/04-review/wasm/intentional-design/bounded-oci-artifact-memory-cache.md).
- Add no request tracing, metrics, dependency or caller-specific alternate
  decoding/collection path. An implementation needs its own exact allowlist,
  independent IMPL plan/final reviews and project checks.

## Acceptance and verification limits

Measure realistic single/multiple-chunk distributions, tiny bodies, sizes around
8192 and the cap, empty chunks, large sliced/shared parents, pending reads and
concurrent exchanges. Include allocations, copied bytes, peak/retained bytes,
downstream Bytes-to-owned conversions and cancellation release. Compare complete
consumer lifetimes rather than only collector return time; no measured benefit
has been established by this audit.

Differentially compare bytes, flags, polling counts, errors and drop order with
the current collector. Include exact-cap followed by error, EOF or more bytes,
and cancellation after a retained first chunk. Existing tests cover initial
pending cancellation, but do not establish those proposed retention properties.

Fresh unchanged-code selections passed five collector, 42 DSL HTTP-call and
27 Elasticsearch bulk tests (74 executions). No candidate implementation, backing
allocation experiment, benchmark, full suite or live network scenario ran.
