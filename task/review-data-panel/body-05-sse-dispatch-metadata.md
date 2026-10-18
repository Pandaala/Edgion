# SSE dispatch metadata representation

Status: deferred optimization for user decision. No runtime change.
Source baseline: 5775da042.

## Evidence

SseDecoder::finish_line classifies each completed line and records its raw range
in BlockBuilder::lines. It updates data/event/id admission counts and, only after
all admission gates pass, publishes malformed/poisoned state. At block dispatch,
parse_block traverses those ranges again, validates and splits fields, and builds
an owned data-range vector. Fragmented lines can materialize during both phases.
The recent line-classification change removes a duplicate validation inside
finish_line but intentionally does not change this dispatch representation.

Caching parsed field/value metadata during line completion could avoid dispatch
reparsing. Adding metadata beside the existing line table retains more capacity;
replacing the table with compact parsed state could remove redundant storage.
Neither layout nor its total allocation/CPU benefit has been measured.

Current admission is tied to physical representation: line-vector capacity,
projected data ranges, raw-slice metadata, line/range counts and event/id presence
all participate. Changing the layout can alter admission and degradation near
these independent limits. Event line_count also passes to RawWindow records.
These are behavior boundaries, not permission to discard accounting because a
new representation happens to use less memory.

## Decision

Choose whether to retain dispatch reparsing or investigate an eager parsed-state
representation with an independently reviewed capacity ledger. This is not yet
an impact-free optimization. Any changed near-limit acceptance needs an explicit
decision; no larger limit or weaker accounting follows from this task.

Unlike body-04, this proposal concerns decoder line metadata and dispatch, not
feeding JSON observers. Parser pooling and retained cross-event arenas are also
outside its scope.

## Acceptance for a selected implementation

- Compare supplemental caches and compact replacement against current physical
  capacity, allocations, repeated copying and CPU on contiguous/fragmented events.
  Include comments, unknown fields, data-heavy events and many small lines.
- Account independently for raw logical/resident bytes, slice handles, all line
  and data capacities, event/id state and transient growth. Preserve hard caps.
  Prove exact admission/degradation equivalence or present changed boundary cases
  for explicit review before implementation acceptance.
- Preserve immediate malformed/poisoned publication only after line admission;
  rejected invalid lines must not publish early. Count malformed_events only when
  the completed block dispatches, including the existing poisoned/data policies.
- Preserve raw byte order and ranges, BOM handling, CR/LF/CRLF continuation, empty
  blocks, comments, unknown fields, colonless fields, one-space removal, duplicate
  event/id semantics and NUL-containing id rejection. Keep semantic data newlines,
  line_count, is_idle and incomplete-tail behavior unchanged.
- Exercise all split positions, exact capacity boundaries, rejected growth,
  poisoned blocks, incomplete EOS, opaque degradation and RawWindow adaptation.
  Metadata must remain valid for the synchronous visitor and any owned handoff.
- Keep existing callback ownership, body backpressure and diagnostic policy.
  No second SSE decoder, full-response aggregation or new hot-path observability.
- Freeze a new IMPL implementation allow-list and obtain independent design and
  final review before replacing the representation.

Sources: edgion-gateway/src/sse/mod.rs (BlockBuilder, finish_line, parse_block,
decoder_metadata_charge and degrade), sse/chain.rs and sse/window.rs
(WindowRecord::from_sse_record). Canonical SSE architecture and capacity-admission
review knowledge remain applicable. Baseline tests do not establish equivalence
of a future metadata layout.
