# Evaluate gRPC mapping selector storage

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `81f551c03`.

## Evidence and decision

`edgion-resources/src/grpc_transcoding/mapping.rs` owns HTTP binding. Cold
CompiledBinding construction prepares path/body field selectors as vectors of
FieldDescriptor handles. Each request clones every selected path vector into
BoundRequest.fields. Query fields instead call resolve_field, which builds a
fresh vector while traversing dotted descriptor names. Successful repeated-list
parameters resolve another vector before merging their value into the existing
field entry; that temporary selector is then discarded. An unknown first name
can fail before any vector allocation, so not every parameter allocates.

Locked prost-reflect 0.14.7 already indexes protobuf and JSON field names.
get_field_by_name/get_field_by_json_name return a descriptor with a cloned
Arc-backed message handle; this is not a deep schema copy or linear field scan.
The candidate concerns selector container allocations and handle ownership,
not replacing the dependency's name index. BoundRequest retains its compiled
binding Arc and owned decoded values across the Gateway body-capture await;
merge later consumes its fields during encoding.

Decide whether realistic shallow mappings justify an inline short-selector
container, or whether to retain Vec. Inline storage avoids a separate allocation
for fitting paths but enlarges every BoundRequest field entry and potentially
cold compiled bindings. The request supports up to 64 mapped field entries and
selectors up to 32 components; wide requests can trade fewer allocations for
larger retained records. A binding-relative index for compiled path selectors
could avoid cloning those paths, but adds a representation split between static
and dynamic selectors and changes merge ownership. No representation or inline
capacity is selected, and no performance bottleneck or gain is measured.

This differs from [grpc-01](grpc-01-query-preflight-decoding.md), which concerns
the discarded whole-query decoded validation buffer, and
[grpc-03](grpc-03-prepared-wire-schema.md), which concerns response wire-tag lookup.
No descriptor graph expansion, global selector cache or lazy request-time index
is proposed. Recursive descriptor graphs must not become expanded path trees.

## Required boundaries

Preserve the combined encoded path/query cap, decoded selector length limit,
32-component limit, field-entry count gate and their current order. Keep
protobuf-name lookup before JSON-name fallback at every component, including
mixed spellings, empty components and unknown fields. Intermediate list/map and
non-message fields remain invalid. ignoreUnknownQueryParameters currently skips
resolve_field errors, not every later scalar/overlap failure; do not broaden it.

Whole-query strict percent/UTF-8 preflight remains after path resolution and
before query field processing, including ignored fields. Preserve repeated-list
value ordering and scalar duplicate rejection, path/body/query overlap tests,
body-all behavior, and ProtoJSON alias collision handling during merge. No new
normalization, parser or schema acceptance is authorized.

A bound request must remain valid after source path/query buffers and the outer
CompiledGrpcTranscoder owner are dropped. The retained binding keeps descriptor
identity alive through body capture and encode. Avoid self-referential borrowed
fields, widened production visibility or dependencies on Gateway types in the
Resources codec. Keep cold compilation rejection and generation ownership,
conversion reservations, response streaming and error dialect unchanged.

## Acceptance for a later implementation

Measure complete bind/encode allocation calls and bytes, peak/live retained
storage and CPU, plus cold binding storage and overlapping generations. Compare
empty/single/many mappings, shallow and 32-component selectors, inline spill,
repeated list parameters, unknown names failing early/late, mixed aliases and
recursive schemas without unbounded expansion. Separate descriptor-handle
clones from container allocations and owned decoded values.

Differential tests must preserve exact encoded values, duplicate/list order,
field/body overlap and alias errors, ignored-query semantics and overlapping
validation failures. Retain tests proving ownership after input and compiled
owner destruction. Require independent IMPL plan/final review and the applicable
project checks. Keep Vec if measured benefit does not justify the wider records
or representation complexity.

Fresh baseline verification passed 35 Resources grpc_transcoding tests and
31 Gateway transcoder tests (66 selected executions). This verifies existing
behavior, not any candidate representation. No implementation, allocation
benchmark or live gRPC integration ran.
