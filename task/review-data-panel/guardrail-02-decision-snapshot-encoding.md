# Guardrail decision snapshot encoding

Status: deferred for user decision. No runtime change.
Source baseline: `e13542457`.

## Evidence and decision

GuardrailResponseProcessor::decide obtains RawWindow::snapshot_all before deciding
whether an empty/current-empty snapshot can pass without a call. materialize_all
scans the canonical window, admits result metadata, text and scratch against its
independent limits, then builds current Strings and copies retained overlap into
context Strings. Guardrail moves those strings into RequestWindow entries and
serializes a complete bounded JSON request before awaiting the decision call.

The mapping moves strings; it does not clone their contents. Do not infer that
map/collect necessarily allocates a second table: collection reuse depends on the
actual implementation and layouts. The demonstrated intermediate ownership is
the semantic snapshot text followed by serialized request bytes, not an established
extra-table allocation or an unbounded-memory defect.

Compare retaining the owned snapshot with a canonical bounded projection or visitor
that can encode some semantic text without materializing it first. A narrower wire
projection change may also be evaluated independently. Any benefit must outweigh
additional traversal, serializer complexity and coupling to window lifetime and
admission. Keep RawWindow policy-neutral: do not embed the Guardrail wire protocol,
introduce a second JSON string decoder or bypass the canonical window owner.
No prototype or measured improvement is established.

## Boundaries and acceptance

- Preserve decoded logical text, bucket order and membership, field/choice
  identities, UTF-8 handling and emitted-context continuity. Context-only buckets
  and raw spans that contribute no semantic text remain part of the existing
  lifecycle; do not reinterpret overlap as unmodified origin history.
- Preserve complete snapshot validation before the current-empty shortcut and
  request encoding. Existing snapshot failures mark the window semantically
  degraded and map to semantic-source. Do not let early serialization overflow
  mask an earlier semantic failure or silently admit inputs rejected by existing
  snapshot limits. Eliminating an allocation does not authorize changing those
  rejection boundaries.
- Preserve independent raw, resident, metadata, semantic, snapshot, overlap,
  replacement and combined-capacity limits. Admission precedes allocation and
  counts simultaneously live capacity, including scratch, tables, strings and
  output. Document result ownership transfer and verify which owners remain live
  across the outbound await; do not assume source-level last use proves release.
- Keep exact request fields, ordering and JSON escaping, exact-cap success and
  cap-plus-one failure. CappedWriter limits logical serialized bytes while writing;
  it does not establish an exact Vec-capacity or process-heap bound. The outbound
  request still needs independent owned bytes before dispatch. Partial encoding
  must never be sent or accepted as a successful decision request.
- Preserve held-head work reservation before snapshot access, request encoding
  before the queue-inclusive call timeout, semaphore scope, retry inputs,
  cancellation, response classification and decision parsing. Keep failure
  policies, release/replace/discard and EOS behavior, aggregate-only observability
  and main-response streaming. No response BodyStorage or new cache is authorized.

Use IMPL with an exact implementation allow-list after user selection. Compare
the current and proposed paths across fragmented/escaped text, multiple buckets,
context-only and empty generations, exact and exceeded snapshot/metadata/output
limits, degraded windows and competing failure conditions. Verify byte-identical
requests and unchanged error categories before measuring allocations, peak visible
capacity and latency. Include pass-heavy traffic and repeated generations; a
smaller intermediate representation alone is not evidence of throughput benefit.

## Sources and scope

- Gateway ai/guards/semantic/response.rs: decide.
- Gateway ai/guards/semantic/protocol.rs: RequestWindow, encode_request, CappedWriter.
- Gateway sse/window.rs: snapshot_all and materialize_all.
- skills/04-review/sse/regression-guards/window-capacity-admission.md.
- skills/04-review/sse/regression-guards/window-emitted-context.md.

This is distinct from [held replacement-header storage](guardrail-01-held-replacement-header-storage.md).
No runtime test or benchmark was run for this documentation-only proposal.
