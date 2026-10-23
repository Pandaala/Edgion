# JSON scan transport ownership

Status: deferred for user decision. No runtime change.
Source baseline: ba032cae9.

## Evidence and scope

The Body variants of JsonPathScanner and JsonSelectScanner in
`edgion-gateway/src/plugins/http/dsl/lang/body_scanners.rs` acquire a JSON scan
permit (limit 32), spawn a parser thread, and receive owned Vec chunks through
`sync_channel(2)`. Their borrowed-slice `push` copies each chunk before sending.
ChannelReader retains the current chunk while serde reads it.

Both scanners require blocking callbacks. RequestBodyView::scan_snapshot in
`edgion-gateway/src/request_body/storage.rs` therefore acquires the generic scan
permit (also limit 32) and uses spawn_blocking even for a small memory snapshot.
That producer borrows memory slices or reads disk into reusable scratch, in
chunks of at most 64 KiB, then calls push. Finish closes the sender and joins the
parser. Known-empty Body handling also schedules blocking finish. String-input
json_get instead uses scan_owned and one run_blocking_scan closure; it does not
use this channel topology.

This establishes per-chunk copying and two execution contexts for the Body path,
not a measured throughput bottleneck, leak or deadlock. The capacity-two queue
is not the entire memory bound: include current parser input, pending producer
input, disk scratch, snapshot backing, serde token scratch and captured output.

## Decision requested

Retain the current ownership boundary, or investigate a storage-owned synchronous
read capability that lets one admitted worker traverse the snapshot and parse it.
An ownership-aware bounded chunk handoff is another candidate. Compare both
against the existing implementation before selecting a design.

The current borrowed push slice cannot simply be retained across threads. Shared
Bytes may avoid copying memory input but pin a larger backing allocation; moving
disk chunks trades reusable scratch for ownership and allocation work. The
existing asynchronous snapshot reader has different I/O deadline and permit
behavior and is not a drop-in serde reader. Do not hide that mismatch with a
block_on adapter or bypass existing storage admission.

Any generic capability belongs to the request-body subsystem; DSL JSON policy
and parsing remain in the DSL scanner. No new BodyStorage, whole-body copy, DOM,
response aggregation, unbounded queue or logging is authorized. This is separate
from DSL 03's outbound-upload cancellation owner and DSL 05/06's capture/key work.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Measure copying, allocation, scheduling
and retained memory for small/large resident bodies, spilled and mixed snapshots,
known-empty input, malformed input and contention. Include cancellation during
admission, reading, sending and parsing; parser disconnection; dropped scanners;
and normal/error completion. Account for pending work after caller cancellation
and retain permits until their actual owners finish. Do not infer bounded cleanup
latency merely from dropping a sender or cancelling an async waiter.

Preserve the selected immutable generation and canonical body ownership. Keep
scan reservation before construction, exact successful settlement and conservative
reservation on errors without accounting. Current traversal charges the whole
fetched chunk after push, including a handoff that discovers parser disconnection;
parser-consumed bytes are not an interchangeable counter. Early rejection or a
pull interface must not silently change refunds or budget exhaustion behavior.

Preserve full-document validation, malformed-input Missing results, individual
and aggregate limits, TooLong versus storage-failure policy, duplicate recovery,
selected descendants under oversized parents and existing cancellation checks.
Review concurrency limits and acquisition order explicitly instead of treating
removal of either worker/permit as an incidental optimization.

At this baseline, 510 Gateway DSL tests passed. They establish existing scanner
and runtime behavior, not acceptance of a new transport topology or performance
improvement. Relevant original contracts are
`skills/01-architecture/02-gateway/dsl-engine-security-model.md`,
`skills/04-review/dsl/regression-guards/bounded-body-text-reads.md`, and
`skills/04-review/http-body/intentional-design/request-body-read-contract-ownership.md`.
