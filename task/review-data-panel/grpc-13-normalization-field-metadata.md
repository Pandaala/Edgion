# gRPC normalization field metadata reuse

Status: deferred for user decision. Priority: P3 performance investigation.
Source baseline: `739a80556`. No runtime representation selected.

## Evidence and value

The ordinary-message path in Resources `grpc_transcoding/normalize.rs` first
resolves every JSON object name, preferring the JSON name and falling back to
the protobuf name. A temporary field-number set rejects aliases. After dropping
that set, null pruning resolves null-valued fields again. The final normalization
loop resolves each surviving field before traversing its scalar/list/map value.

For a successful ordinary object, a non-null field undergoes two resolution
sequences; a removed null undergoes two; a retained singular Value/NullValue null
undergoes three. Each sequence may use a fallback lookup. Invalid inputs may
return earlier. Exact WKT message shortcuts follow separate paths; these counts
are not universal per-field or per-message costs. Non-null values already bypass
the pruning lookup, and known collection cardinality already avoids redundant
map/list classification. Do not propose those completed changes again.

Locked prost-reflect 0.14.7 stores protobuf and JSON names in indexed HashMaps.
A successful lookup returns a FieldDescriptor containing a cloned Arc-backed
message handle and field index. This is repeated hashing/handle work, not a
linear schema scan, deep schema copy or proven new heap allocation per lookup.

Consider retaining current lookups, investigating request-local resolved metadata
through the existing passes, or preparing a compact immutable normalization shape
per compiled generation. No variant, inline capacity or cache is selected. There
is no measured bottleneck or net benefit; keeping the current path is valid.

Request-local storage adds its own allocation/capacity or enlarges inline state.
Retaining descriptors through recursive normalization can overlap ancestor and
child metadata, unlike the field-number set now released before traversal.
Compile-time metadata duplicates some existing indexed reflection data and adds
construction scratch, retained generation memory and hot-reload overlap. Current
descriptor admission caps inventory at 1024 messages and 8192 fields; those caps
alone do not prove a proposed representation fits an acceptable memory budget.
Recursive/shared schemas need bounded identities rather than expanded trees or
owning reference cycles. Add no global or lazily populated request-time cache.

## Acceptance boundaries

- Validate all names and alias collisions, including null aliases, before any
  null removal or scalar rewriting. Preserve JSON-name-first fallback semantics.
- Finish the existing null-pruning pass before recursive conversion can fail.
  Preserve current object iteration, error and mutation ordering; do not fuse
  validation into a mutating walk and call it equivalent.
- Do not zip pre-pruning metadata blindly with post-pruning values: removal can
  shift positions. Demonstrate correct name/field association for every survivor.
- Preserve exact WKT dispatch, ordinary similarly named messages, singular
  Value/NullValue sentinels, repeated/map null removal and element-null rejection.
  Preserve list/map entry accounting and first depth/node/byte-limit failure.
- Re-establish the reviewed exchange peak and cold-generation bounds before
  selecting storage. Do not reduce or increase the existing 32 MiB reservation,
  64 MiB process admission, or other limits under a lookup optimization.
- Test invalid names, null/non-null alias combinations, mixed removals, recursive
  shapes, map/list/scalar fields, first failing child and limit boundaries.
  Measure hashing/refcounts, allocations and peak retained capacity separately
  for narrow/wide/deep inputs and hot reload; compare against current indexes.
- Freeze the allow-list and obtain independent IMPL plan/final review. Reuse
  canonical reflection/normalization policy; add no traffic tracing or metrics.

This concerns request schema normalization, separately from [wire-tag shape
preparation](grpc-03-prepared-wire-schema.md), [binding selector storage](grpc-10-mapping-selector-storage.md)
and the [normalized-text reflection bridge](grpc-11-request-reflection-bridge.md).
Those decisions remain unselected.

Sources: `edgion-resources/src/grpc_transcoding/{normalize,codec,descriptor}.rs`,
locked prost-reflect `descriptor/{api,mod}.rs`, and the Gateway transcoder's
`run_request` consumer. The [admission and memory ledger](../../skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md)
remains authoritative. Existing ordering/null tests are baseline evidence, not
proof of a new metadata representation.

Fresh unchanged-source verification passed 35 Resources grpc_transcoding and 29
Gateway plugin transcoder tests (64 total), formatting and workspace all-target
check/Clippy with existing warnings. No prototype, allocation/latency benchmark,
full suite or live transport test ran.
