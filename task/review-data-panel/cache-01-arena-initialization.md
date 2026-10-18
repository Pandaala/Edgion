# Response-cache arena initialization timing

Status: deferred design decision. Source baseline: `56f77ccd7`.

## Evidence

Response-header capture reaches `response_cache::lifecycle::capture_headers` and
`RequestState::capture`, then `Pending::create` in
`edgion-gateway/src/response_cache/storage.rs`. After policy/freshness validation
and full body/metadata/control lease admission, creation reserves exactly
`store.body_cap` bytes and calls `arena.resize(store.body_cap, 0)`. The default
body cap is 1 MiB. A short or subsequently abandoned fill therefore initializes
the full configured arena before the first body callback.

`Pending::push` checks the next used length against `arena.len()` and copies into
the initialized slice without allocating. `Entry::decode` rejects used lengths
beyond the initialized arena; `Reader::as_ref` exposes only `[..used]`. The
reader owns the entry through Bytes::from_owner, retaining the full capacity
lease even after eviction and while downstream slices survive. These ownership
and accounting properties must remain intact.

The current code performs full-capacity initialization. This is not evidence of
a measured CPU, resident-memory or latency problem: allocator and operating
system behavior affect actual page touching and write costs. No benchmark was
run for this proposal.

## Decision

Choose whether to keep eager initialization or investigate a safe incremental
initialization design. The latter could reserve and charge the same full
capacity before capture, leave Vec length at zero, then append only accepted
chunks within that capacity. It must not introduce unsafe uninitialized reads,
per-chunk allocation, growth beyond admission, a second cache or response
BodyStorage. Simply deleting resize is incorrect because current push bounds
and slice writes depend on the initialized length.

This would change when memory is first written and potentially when page faults
occur: work currently paid at header capture can move into body callbacks.
Capacity accounting would remain conservative, not become used-byte accounting.
The ownership of initialized length versus used length and corruption checks
also needs an explicit reviewed design. These performance and representation
choices prevent treating it as an unconditional behavior-free optimization.

## Acceptance for a future implementation

- Retain full lease admission before allocation, exact-capacity refusal, bounded
  metadata, and allocation-free accepted body pushes, including the first push.
- Prove empty EOS, exact-capacity bodies, overflow, repeated EOS, cancellation,
  abandonment, expiry and epoch retirement preserve publication and cleanup.
- Preserve decode corruption rejection and show readers never expose spare
  capacity or bytes beyond the initialized body. Retained slices must keep the
  full arena charge after entry replacement or eviction.
- Compare header-capture and chunk-processing costs for empty, tiny, near-cap
  and abandoned responses, including concurrent pending fills. Measure total
  work and latency distribution; report page/resident-memory observations
  separately from logical ledger charges and allocator calls.
- Reuse storage allocation/lease tests, duplex H1/H2 terminal tests and actual
  request-tail binding/reload tests. The existing reload fixture failure tracked
  in `verification-01-cache-policy-fixture.md` remains a separate known issue.
  Obtain IMPL plan/final reviews and run repository gates before committing code.

Original constraints:
`skills/01-architecture/02-gateway/00-overview.md` (bounded response caching) and
`skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md`.
This task changes no production code or cache contract.
