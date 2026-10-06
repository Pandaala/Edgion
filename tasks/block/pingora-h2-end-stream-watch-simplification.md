# Pingora HTTP/2 END_STREAM Watch Simplification

Status: **blocked** on upstream h2 trailer validation. Updated: 2026-10-06.
Priority: P2. Scope: the Edgion Pingora fork's response-completion watcher.

## Blocker

The audited minimum and local dependency resolution are both **h2 0.4.19**.
Its trailer receive path does not provide the guarantees needed for further
watcher removal:

- Reject all trailer pseudo-fields, including pseudo-only and mixed blocks,
  before publishing a successful trailer event.
- Reject oversized decoded trailer blocks, including fragmented CONTINUATION
  blocks, before closing the receive side or queueing trailers.
- Preserve observable rejection after a same-burst `RST_STREAM(NO_ERROR)`;
  invalid trailers must never become clean EOF or a successful trailer map.

Retain the watcher, byte-delivery checks, reset/GOAWAY invalidation, permanent
error latches, and fail-closed cache admission. Do not vendor or privately fork
h2, add a second HPACK decoder, or infer validity from wire END_STREAM alone.
The watcher cannot recover pseudo-fields discarded by h2; that upstream
limitation remains accepted while waiting for the normal upstream fix.

## Resume gate

1. Adopt a normal upstream h2 release that fixes the decoder contract, recording
   the minimum supported and actual resolved versions. Obtain a new independent
   design review before further production simplification.
2. Enable and pass pseudo-only, mixed, oversized, fragmented-CONTINUATION, and
   same-burst-reset tests across async body, poll body, and direct trailer reads.
   Preserve terminal decisions across repeated reads and accept valid empty
   trailers; finish the deferred shared trailer/error-latch work.
3. Prove complete byte delivery, Content-Length handling, reset order, GOAWAY,
   flow-control drop, local invalidation, cache admission, connection reuse, and
   resource release with and without the watcher at both dependency boundaries.
   Only remove responsibilities proven redundant; kill a mutation for each
   retained guard and obtain an independent final review.
4. Revalidate historical follow-ups against current code: terminal HEADERS must
   be independent of prior DATA; ordering/error-latch tests must be deterministic;
   document per-failure connection disposition and only claim enabled, verified
   contracts. Historical audit tables are not current defect inventories.

## Completed stage

On 2026-10-06, the scanner's mutually exclusive payload states were consolidated
without removing integrity guards. Production and test layouts decreased from
136 to 96 bytes on macOS aarch64. Astra independently approved the design and
final implementation. Focused H2 tests passed (146 passed, 16 intentionally
ignored), as did proxy regression, 12 mutation checks, compilation, Clippy, and
documentation validation. This does not satisfy the upstream-dependent removal
scope or the ignored decoder contracts.

Local streaming/multiplex microbenchmarks improved; terminal padded DATA and
GOAWAY microcases added approximately 5-8 ns. No gateway-throughput claim is made.
The implementation is local to Pingora; Edgion's dependency was not updated.

## Evidence and ownership

- [Edgion upstream-limitation policy](../../skills/04-review/h2-grpc/regression-guards/h2-trailer-decoder-upstream-limitation.md).
- Pingora canonical pending records: `edgion-changes/pending-issues/h2-trailer-validation.md`
  and `h2-terminal-headers-completion.md` in the same directory.
- Completed implementation, Astra approval, and performance evidence:
  `edgion-changes/review/h2-grpc/h2-payload-state-consolidation.md`.
- Verification provenance and reproducible commands:
  `edgion-changes/verification/test-matrix.md` (2026-10-06 entry).
