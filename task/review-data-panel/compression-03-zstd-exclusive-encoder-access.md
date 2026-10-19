# Evaluate exclusive access to the Zstd encoder

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Edgion baseline: `851d9516c`.
Inspected Pingora fork: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence and scope

The downstream ResponseCompression module initializes all algorithm levels from
the selected compression level. Eligible known-length, non-SSE responses can
therefore negotiate Zstd. In the fork's
`pingora-core/src/protocols/http/compression/zstd.rs`, ordinary
Compressor::encode takes `&mut self` but calls `self.compress.lock()` on every
chunk and finalization. The Mutex wraps the encoder because the stored
`Box<dyn Encode + Send + Sync>` requires Sync and the encoder is not Sync.

Keep that Mutex representation and evaluate using its exclusive `get_mut()`
accessor inside encode. The locked parking_lot 0.12.5 / lock_api 0.4.14 accessor
returns the inner mutable reference without locking; the outer mutable borrow
provides exclusivity. This would remove lock/unlock work, not a measured source
of contention. No throughput or allocation benefit has been established.

DictionaryCompressor has a similar lock, but also calls build_header(&self)
after borrowing the encoder. Any conversion requires a separate field-borrow
review preserving first-header construction and ordering. No dictionary setup
call exists in Edgion's current gateway sources; do not activate that feature or
claim it is the ordinary reachable path. Compression-01 preference storage and
compression-02 empty-output retention are separate decisions.

## Boundaries and decision

Defer until the user authorizes an exact external Pingora-fork scope and adoption
plan. Do not edit Cargo's dependency checkout or change the workspace revision
as incidental cleanup. This task grants no external mutation authority.

Preserve the Send + Sync contract, output bytes/order, buffer ownership, errors,
input/output counters, finalization and cancellation cleanup. Retain the duration
measurement boundary; measured duration naturally changes with executed work.
Do not remove the Mutex type, add unsafe implementations, pooling, extra flushes,
response aggregation, negotiation changes, logging or metrics.
Keep Edgion's captured configuration and known-length/non-SSE eligibility gate.
Revalidate footer-before-trailer and exactly-once completion against the actual
fork rather than relying on historical test results in review knowledge.

## Acceptance for a later implementation

Run the fork's Zstd encoder and compression-module tests, retaining trait-bound
compilation checks. Compare concatenated encoded output and decoded content for
empty, tiny and large chunks, compressible/incompressible input, final empty
input, retained outputs and cancellation; preserve encoding-error propagation.
Verify terminal/footer ordering through the real module and retain Edgion's
enabled/disabled, origin/cache and known-length eligibility tests. Evaluate
dictionary behavior separately if that owner enters the frozen scope.
Measure CPU at realistic chunk sizes and concurrent response counts before
claiming end-to-end benefit; unrelated encoder allocations remain unchanged.

Fresh Edgion verification passed 26 response-filter and two module-registration
tests. These do not execute the proposed fork change; no external fork tests,
allocator benchmark or live traffic benchmark ran for this investigation.
