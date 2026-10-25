---
name: window-capacity-admission
description: Review RawWindow allocation peaks, SliceChain append admission, and bounded semantic materialization.
knowledge_type: regression-guards
---

# Window capacity admission precedes allocation

## Conclusion and scope

The canonical gateway SSE window contract requires independent raw, resident,
metadata, semantic, replacement, overlap, and combined-capacity bounds. Admission
must occur before the relevant allocation, using checked arithmetic and explicit
capacities. A final-size check after building an append or replacement plan does
not bound the construction peak.

The model includes raw payload, raw handle tables, bucket/fragment/range capacity,
overlap, owned record metadata, replacement String/Vec capacity, staged vectors,
token scratch, result tables/text, edit descriptors, and encoded output. Count
every simultaneously live allocation once, including unused container capacity.
It measures visible capacities, not allocator size classes, RSS, fixed library
control blocks, external transport buffers, or arbitrary caller-owned Bytes
backing. Retained Bytes are normalized; returned results transfer to the caller.
Callout, decoder-batch, and Guardrail callback-output limits remain independent.

## Rationale and evidence

In `edgion-gateway/src/sse/chain.rs`, `inspect_append` computes additional storage
before `prepare_append` allocates. AppendSlices reuses its existing raw table;
AppendCoalesced writes into the admitted arena at commit without a compact scratch
copy. Window bucket growth stages empty destination containers and moves existing
elements; `bucket_metadata` caches committed capacity and is reset/recomputed
when a generation completes. Old range allocations are neither cloned nor
recounted on every append. `try_retain` rejection preserves the original input.

In `edgion-gateway/src/sse/window.rs`, JSON string decoding uses an explicit raw
token buffer plus a visitor shared by counting and writing. Serde's deserializer
scratch and intermediate decoded String are not part of this hard-bound path.
String encoding still reuses serde_json with counting and capacity-bounded writers.
Replacement edits contain direct bucket indices, sort without heap scratch, and
write into one admitted output buffer. Length-only preview borrows text and
allocates neither output nor overlap. Snapshot accounting includes every returned
context/current string and the result table together with live storage/scratch.

## Alternatives and limits

The previous large-input test treated the input handle table as free memory.
Input metadata is now admitted as well: oversized input tables are rejected even
if coalescing would make retained metadata small. The regression still verifies
bounded multi-handle coalescing and empty append behavior with admissible inputs.

The old task also mentioned repeated linear replacement lookup. Current baseline
code already sorted and zipped replacements; that claim described a rejected
attempt, not a separate reproduced defect. Repeated bucket cloning and untracked
sort/decoder/preview allocations were current defects. No claim is made that all
semantic scanning across successive stats calls is amortized linear.

## Verification and re-evaluation

Regression tests independently enumerate owned capacities for exact-cap and
cap-minus-one cases covering token decoding, whole snapshots, reused raw tables,
coalescing, append-to-arena storage, and terminal replacement with spare input
capacity. They also check returned record pointers and retained range identity.
JSON semantics are compared with serde_json outside the bounded path.
Verified on 2026-09-23: the gateway library filters `sse::` (74 tests) and
`plugins::http::guardrail::response::` (18 tests) passed. Workspace all-target
check and Clippy, formatting, agent-doc validation, SSA force, metrics inventory,
Pingora source policy, and Gateway API transform contract checks passed.
Independent read-only code and knowledge review returned LGTM.

A standalone allocation probe importing the actual substrate also observed zero
allocations for borrowed preview, Discard, and oversized-input rejection. Its
escaped-token snapshot used two allocations, and terminal replacement used two
versus three for nonterminal replacement, with no reallocations in these cases.
This supplementary probe validates selected paths, not every allocator behavior;
the committed boundary and identity regressions remain the durable checks.
Re-evaluate on allocator/container changes, serializer changes, new scratch,
alternative token sources, or altered snapshot ownership.

## Resolution references

- `sse-window-strict-residency-and-continuity` — corrected and verified on
  2026-09-23; independent review: LGTM.

## Coalesced append inspection

`SliceChain::inspect_append` counts nonempty incoming handles only for sliced
storage, where the count selects coalescing or slice-table admission. An already
coalesced chain needs only the checked total input length and its existing arena
capacity; it skips the unused handle-count traversal. Logical admission still
precedes representation-specific checks, and every resident/metadata layout
value and error boundary remains unchanged. This removes a source-level scan,
not an allocation, and establishes no measured throughput improvement.

Existing one-byte-stream, large-record/empty-append, exact handle-limit and
coalesced-arena admission regressions cover this boundary. Re-evaluate if the
coalesced layout gains a handle-dependent policy or representation.
