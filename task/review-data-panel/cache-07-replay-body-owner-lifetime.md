# Evaluate response-cache replay body ownership

Status: deferred for user decision. Priority: P3. Source baseline: cee094be7.
No runtime or dependency change.

## Evidence and decision

Store::lookup in response_cache/storage.rs returns a Hit with an entry Arc and
Bytes::from_owner(Reader). Reader independently owns an Arc to the same Entry
and its READER_CONTROL lease. These are shared handles, not duplicate body
payloads. Hit also carries a decoded header and its separate header-work lease.

Before delivery, retain_header_charge needs the entry for contract validation
and identity-specific eviction, and moves the header charge into RequestState.
deliver_hit then reads freshness for Age, writes the header, and sends 16 KiB
slices of the parent Bytes. The parent Bytes and Hit.entry remain in scope
through the separate EOS write and finish_body await.

Possible changes include releasing the extra entry owner after its last use,
releasing the parent Bytes after chunk submission, or moving a final chunk
instead of cloning a slice. Choose only after comparing reference-count work,
ownership complexity and actual retention. Keeping the existing straightforward
loop is a valid outcome. No prototype or performance result selects an option.

Dropping Hit.entry alone cannot release the Reader-pinned arena. Dropping parent
Bytes releases Reader and its control lease only after all retained slices are
gone. The cache index and other readers can still retain Entry. Consequently,
earlier release does not imply immediate arena reclamation or lower RSS. Empty
payloads and failed writes need explicit treatment; do not silently alter their
admission charges as part of an ownership rewrite.

## Boundaries and acceptance

Keep the canonical cache, complete entry/header checks, identity-specific
eviction, origin/epoch fences, zero-copy bounded chunks, backpressure, exact
payload order and separate EOS semantics. Retain the same output deadline and
downstream error mapping. A postheader failure must never fall back to origin.
Conditional and range requests retain their existing cache bypass.

Header work remains charged through context teardown because Session may retain
response_written. Body/entry/control leases must cover every surviving owner,
including slices retained after eviction or replacement. Reader counts describe
the Reader owner lifecycle; any earlier transition needs evidence that it still
matches the ledger and residue contract. Do not drop a lease while retaining its
charged storage, or add a response BodyStorage or another cache owner.

Verify empty, one-chunk, exact-boundary and multi-chunk replay; delayed header,
body, EOS and finish operations; failure and cancellation at each await; and
H1 flush versus H2 accepted-EOS behavior. Retain slices while evicting/replacing
entries and retiring epochs, then assert admission totals, reader counts and
final release after each independent owner drops. Existing success/failure
tests are baseline evidence, not proof of a new handoff's concurrency behavior.

Measure reference-count operations, wrapper/slice allocations, control charges,
retained arena capacity and total live ownership under slow consumers and
eviction. Distinguish logical byte length from backing allocation, and avoid
attributing index-owned memory to the replay handle alone. Preserve the cache's
fixed metadata and body bounds; this task permits no new metrics or payload logs.

This is separate from [replay header capacity](cache-05-replay-header-capacity.md)
and [prepared Content-Length](cache-04-prepared-content-length.md).
The original contract is [bounded public-origin response cache](../../skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md).

Fresh unchanged-code verification passed 21 storage and seven lifecycle unit
tests using in-memory transport fixtures. No live origin, full suite, allocation
benchmark or proposed ownership implementation was run.
