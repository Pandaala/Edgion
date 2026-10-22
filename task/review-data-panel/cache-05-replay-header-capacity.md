# Evaluate response-cache replay header reservation

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `6859cf7ff`. No runtime change.

## Evidence

`response_cache::lifecycle::prepare_hit` reaches `Store::lookup` and
`Entry::decode` in `edgion-gateway/src/response_cache/storage.rs`. Every decode
constructs `ResponseHeader::build(200, Some(policy::MAX_FIELDS))`, where MAX_FIELDS
is 64, before traversing the stored metadata. Sparse entries use the same hint.
The locked Pingora revision `1aaa45cc70430bc782990bc20ddee3fef36d6b99`,
`pingora-http/src/lib.rs`, passes that hint to both the ordinary HeaderMap reserve
and the CaseMap capacity constructor. The hint is not an exact allocation size:
container rounding and duplicate-value storage remain implementation details.

Before decoding, lookup admits HEADER_WORK (64 KiB) and READER_CONTROL (1 KiB).
The delivered header charge moves into RequestState because Session can retain
the header. Entry/body ownership remains independently pinned through Reader and
Bytes::from_owner. Smaller actual header allocations would not by themselves
reduce these conservative admission charges or permit additional readers.

`pg_request_filter::respond_from_cache` runs the shared cached synchronous header
tail, async response filters and the precommit hook before delivery. Lookup adds
Content-Length and delivery updates Age. Headers can grow after decode; retaining
spare capacity may avoid later allocations. Optimizing decode in isolation can
therefore move cost into later callbacks rather than reduce total work.

The existing storage test
`header_decode_and_owner_wrapper_requested_capacity_fit_precharged_work`
exercises a 64-field entry and bounds requested allocation bytes. It does not
establish the best reservation for sparse entries or downstream header mutations.
No alternative implementation or performance benchmark was run.

## Decision and acceptance

Compare the fixed maximum hint with a smaller bounded hint, a complete metadata
counting prepass, or a cold-recorded field count. Select a representation only if
whole-replay measurements justify it. A prepass adds traversal; a stored count
adds derived state and integrity obligations. Field records and unique names are
different counts, and repeated values still require storage. Removing the case
map is a separate behavioral decision, not part of this capacity candidate.

Measure sparse, dense, repeated-name and near-limit metadata, corrupt tails and
headers that grow during response plugins. Report allocation calls, cumulative
requested bytes, retained capacity and CPU separately. Include Content-Length,
Age and final H1/H2 serialization, rather than ending measurement at decode.
Do not claim throughput or resident-memory gains from a smaller hint alone.

A future independently reviewed IMPL must preserve full metadata validation,
including malformed suffixes after useful fields; bounds, duplicate ordering,
current casing and public-origin/Vary/freshness contracts; identity-checked
eviction; epoch fencing; refusal cleanup; and retained header/body leases.
Admission must still precede allocation. Reducing HEADER_WORK requires a separate
complete accounting argument and is not authorized by this task. Preserve ordinary
streaming, terminal ordering and canonical access-log ownership; add no hot-path
tracing, second cache or response BodyStorage.

This is separate from cache-01 arena initialization, cache-02 Age construction,
cache-03 repeated Vary validation and cache-04 prepared Content-Length.
Canonical constraints:
`skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md`.

Fresh unchanged-code verification passed 21 storage and seven lifecycle tests
serially, formatting check, workspace all-target check/Clippy and agent-doc
validation. Existing unrelated warnings remain. These selections do not resolve
the broader admission fixture tracked in verification-01 or establish live
transport acceptance. No full suite, live integration or benchmark ran.
