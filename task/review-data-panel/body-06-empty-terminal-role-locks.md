# Evaluate empty-role terminal finalizer lock elision

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `cd42b7c6b`.

## Evidence and potential benefit

`response/driver.rs::ResponseProcessorDriver` owns one Tokio mutex for body and
trailer processor state. Its `finalize_semantic_incomplete` and
`finalize_trailers_incomplete` acquire that mutex before checking cancellation
and visiting their target containers, including when those containers are empty.
`finalize_ai_incomplete` also locks before checking its optional AI processor.

Universal logging calls `ai/http::finalize_semantic_for_logging` and
`pg_upstream_response_trailer_filter::finalize_incomplete_trailer_companions`.
The semantic helper uses any existing driver, without a semantic-presence gate.
Trailer cleanup has its existing once-only context claim but no role-presence
gate. AI finalization first requires AI state, which alone does not establish
that the driver contains an AI processor. An entirely empty processor set
already produces no driver; this proposal concerns nonempty drivers whose
particular finalization role is absent.

Constructor-time presence flags could avoid some terminal mutex acquisitions.
The existing immutable `trailer_count` is another candidate for trailer gating.
No implementation, net CPU benefit, contention frequency or throughput gain has
been established. This is terminal request work, distinct from the per-chunk
read-only runtime-chain proposal in body-01.

## Why this remains a decision

Even an empty-role call currently waits for the shared state mutex. An early
return removes that synchronization with an outstanding body or trailer lease.
Determine whether any supported production terminal lifecycle depends on that
wait before later accounting, owner cleanup or log publication. Source inspection
has not established a reachable race or proved the wait universally redundant.
Synthetic concurrent calls alone cannot establish production reachability.

The context freezes builder slots into the driver once. Current production lease
consumers inspected visit processors without changing container membership, but
`BodyExecutionLease::processors` exposes a mutable BodyProcessorSet and
`TrailerExecutionLease::companions` exposes a mutable Vec. Cached membership
therefore needs an explicit invariant across all consumers and future lifecycle
changes; do not present this API capability as observed dynamic insertion.
Include constructor metadata and struct layout cost in any proposed benefit.

Retain group-specific cancellation, lease-before-callback construction, cleanup
before access logging, first terminal publication and once-only log emission.
Do not combine distinct terminal stages or change response ownership to amortize
locks. Preserve the original cancellation contract in
`skills/04-review/plugin-runtime/regression-guards/response-callback-downstream-cancellation.md`.

## Acceptance evidence

Trace supported callback/logging overlap for normal completion, aborted and
unpolled callbacks, local replies, cache paths and retries. Add held-lease tests
that explicitly distinguish empty-role waiting from an early return, then relate
those observations to production ownership. Exercise absent/present AI, semantic,
ordinary and trailer combinations, cancellation of each group, first publication
and repeated finalization. Retain real transport cancellation acceptance if the
change affects that boundary; passing unit tests alone is not transport proof.

Measure terminal CPU/lock cost and request storage for representative short and
streaming responses before adopting extra metadata. Preserve canonical logging,
redaction, body bounds and observability restraint. No new metrics are needed.
