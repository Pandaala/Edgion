# JSON object key encoded-length ownership

Status: deferred for user decision. No runtime change.
Source baseline: f5953657f.

## Evidence and scope

In `edgion-gateway/src/plugins/http/dsl/lang/body_scanners.rs`,
`BoundedMapKeyVisitor` admits a retained key through `encoded_string_fits`.
`SelectedMapKeyVisitor` uses the same gate only when encoding the current object
requires retaining that key. The gate first rejects an overflowing or excessive
raw byte length plus two quotes, then counts actual JSON encoding through serde's
writer and CountingWriter. Admitted selected keys can reuse the immutable trie
key Arc instead of allocating another raw key.

`BoundedMapAccumulator::insert` receives the raw key and counts its encoding
again for entry/duplicate accounting. `finish` eventually writes accepted keys
into the bounded canonical object. Thus an accepted retained key may be counted
twice and later encoded for output. This is repeated encoding work, not evidence
of three payload copies or three passes for every parsed key. Rejected keys and
keys not retained for object reconstruction follow different paths.

## Decision requested

Retain the current interface, or investigate carrying an admitted encoded length
alongside the owned key into the accumulator. The metadata must describe the
exact immutable raw key and canonical serializer; construction must prevent
mismatched or unvalidated lengths from bypassing admission. Both ordinary object
and selected-object retention paths need one coherent representation.

Known trie-key lengths could alternatively be prepared at VM construction, but
unselected input keys still need runtime admission. Measure added plan/per-key
metadata and branches against repeated counting work. Preserve canonical key Arc
reuse. Do not add a process-global or per-request key cache, or retain encoded key
payloads merely to avoid counting without a separate memory evaluation.

This is separate from DSL 05's capture projection and the completed borrowed
capture conversion. No throughput or allocation improvement has been measured,
and no representation change is approved by this task.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Compare common ASCII, escape-heavy and
multibyte keys; known/unknown trie members; repeated keys; small and maximum
bounded objects. Include metadata retention, cold preparation and malformed or
over-limit inputs, not only successful counting loops.

Preserve the raw-length lower-bound rejection, exact encoded lengths, zero/exact/
over-limit cases, duplicate-entry subtraction and comma accounting. Keep the
recoverable overflow marker for the same key distinct from irreversible overflow
across different keys or an overlong key. Never treat an earlier oversized value
as permanently fatal when a later duplicate can replace it under current rules.

An overlong parent key must not suppress independently selected descendants.
Keep canonical sorted object output, complete-document validation, malformed-JSON
discard, individual/aggregate capture limits, worker ownership, snapshot generation
and scan accounting unchanged. No full DOM, second BodyStorage, new response
aggregation or logging is authorized.

At this baseline, 506 Gateway DSL tests passed. Existing tests cover encoded
count/write agreement, canonical plan-key identity, duplicate length/comma
updates, same-key overflow recovery, irreversible overflow and parent/descendant
selection. They do not establish a new metadata contract, measured savings or
full live transport behavior. Original contracts are
`skills/01-architecture/02-gateway/dsl-engine-security-model.md` and
`skills/01-architecture/02-gateway/dsl-engine.md`.
