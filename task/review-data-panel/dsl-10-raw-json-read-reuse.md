# Reuse across strict raw JSON reads

Status: deferred for user decision. No runtime change.
Source baseline: 065905ef7.

## Evidence and scope

Gateway `lang/json_builtins.rs` handles JsonRead(Str) by calling the canonical
leaf `json::parse_raw`, discarding the RawDocument, and returning the original
Arc string. Each subsequent JsonGetRaw, JsonGetString or JsonKeys call parses
its source again, even when the same immutable string is reused. N successful
read calls therefore invoke parse_raw N times; an explicit preceding json.read
adds another invocation. Failures and early returns can stop this sequence.

`edgion-dsl/src/json.rs::RawDocument` borrows RawValue spans from caller-owned
text and retains a Vec of decoded top-level member names. Its get lookup is
linear. Each parse validates the complete value recursively, including decoded
duplicate names, escaped Unicode, depth and node bounds. RawValue parsing and
nested validation can scan source regions repeatedly within one invocation.
These are not N full DOM payload copies: raw values remain borrowed spans.

Repeated reads repeat validation and member preparation. Existing Gateway JSON
reconstruction tests demonstrate multiple reads from one encoded input, but do
not measure a workload bottleneck. No allocation or throughput benefit has been
measured for a replacement design.

## Decision requested

Choose whether to investigate a bounded batched-read operation or a prepared
validated view with explicit ownership and admission, or retain independent
parsing. No design is selected. The current public contract uses encoded Str
values, not a JSON-specific Value variant. This task does not authorize changing
that contract, introducing a global/request-body cache, or replacing the parser.

The existing json_select path is not a drop-in replacement for these strict
raw operations; its selection, result and duplicate/error semantics need an
explicit comparison. DSL 05 concerns transfer of selected captures, while DSL 07
concerns scanner transport. Neither resolves reuse of this strict RawDocument.
A redesign of recursive RawValue traversal is also separate from reuse across
calls and must not be smuggled into the first implementation batch.

## Acceptance before implementation

Use IMPL with a frozen source allow-list. Compare one versus many reads, small
and bounded-large objects, escaped names, nested values, hits/misses, and reuse
after json.set_raw/json.set_string. Measure construction, repeated work, metadata
and retained source memory separately. Include workloads with no reuse so that
index preparation and retention costs remain visible.

RawDocument currently borrows its source. A retained view must own/pin the exact
immutable generation and its validation profile without unsafe lifetime extension
or raw-address identity caching. Bound metadata construction, entries, retained
bytes and cleanup on success, error and cancellation. A new key index must retain
member order and cannot use a weaker duplicate-key policy.

Preserve full-document validation before returning an early member, malformed
trailing-data rejection, decoded duplicate aliases at every depth, surrogate
validation, syntax-first precedence, exact raw number/nested spelling, and
encoded null versus absent nil. Keep non-object/missing/member-type results,
key order, UTF-8 output limits and independent returned-string ownership.

Existing parsing, allocation and copy work charges are conservative and
non-refundable. Removing actual work does not authorize making previously
rejected scripts pass. Preserve each observable call's limits, error ordering
and budget contract, or obtain a separate explicit decision for new accounting.
A saved total cost alone may not reproduce partial traversal failure precedence.
Any accounting metadata must itself be bounded and validated.

Keep source/precompiled behavior and Gateway trust-boundary checks aligned.
Do not modify Resources DOM parsing, main response streaming, BodyStorage,
body-generation/publication rules or request-level observability. Any new leaf
API must remain runtime-independent and serve the existing canonical JSON owner.

Fresh baseline verification passed 515 Gateway DSL tests. Existing exact-number,
absent/null, escaped-duplicate, work-limit, body publication and source/wire tests
are starting evidence, not acceptance of a new parsed representation or API.
Original authority: `skills/04-review/dsl/regression-guards/bounded-json-reconstruction.md`
and the strict JSON section of
`skills/01-architecture/02-gateway/dsl-engine-security-model.md`.
