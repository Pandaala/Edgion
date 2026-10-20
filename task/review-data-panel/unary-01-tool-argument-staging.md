# Decide unary tool-argument serialization staging

Status: deferred for user decision. Scope: explicitly selected non-streaming
Chat-to-Anthropic response conversion.

## Evidence

`translate_unary` in
`edgion-gateway/src/ai/conversion/chat_to_anthropic/response.rs` validates the
bounded source document and supported block shapes before publishing validated
usage. It then serializes each tool input object through a `LimitedWriter`.
The writer's limit is the configured output limit minus the already serialized
argument bytes. Each resulting Vec is moved into a String with `String::from_utf8`;
that conversion validates UTF-8 but does not copy the payload.

`UnaryFunction.arguments` retains each String until the derived outer response
serializer writes it as a JSON string into another bounded writer. The intermediate
argument buffers therefore coexist with the parsed DOM and growing final output.
Tool IDs and names already borrow from the DOM. `budget::output` moves the final
buffer into an output owner that shares the reservation with the exchange;
downstream output clones and slices can retain that lease after exchange cleanup.

The intermediate bytes represent compact serialization of the parsed Value,
not a slice of the original JSON. Duplicate object keys have already resolved
with last-wins semantics. Original whitespace, escape spellings and duplicate
entries cannot be substituted for the canonical inner serialization.

## Decision

Keep the bounded staging, or investigate a borrowed serialization adapter that
emits canonical inner JSON through the outer JSON-string escaping layer without
retaining every tool's encoded argument String. This is a potential reduction in
temporary storage and allocation, not a demonstrated throughput or memory gain.
It introduces serialization and accounting complexity, so it is not a drop-in
clone removal.

Do not change arguments into an embedded JSON object, concatenate unescaped
source fragments, or use a fallible Display path without explicit error handling.
Do not remove cumulative inner-byte accounting merely because an outer byte cap
also exists. An adapter must specify where inner and outer bytes are counted,
where failures stop emission, and how any scratch storage is bounded.

## Required acceptance

- Preserve both encoding layers: the compact inner JSON bytes and their escaped
  representation as an outer JSON string. Verify duplicate-key resolution,
  object ordering, numeric rendering, control escapes, quotes, backslashes and
  permitted multibyte UTF-8 against the current serializer.
- Preserve tool indexes, source block order, IDs/names, mixed text/tool output,
  tool-only null content, empty tool-call omission, stop mapping and usage.
- Preserve complete document validation before usage publication. Invalid input
  publishes no usage; valid usage survives later argument/output overflow and
  cleanup. Keep the current fixed error classification and precommit behavior.
- Keep independent cumulative argument and final-output byte caps, checked
  arithmetic, parsing complexity bounds and existing deadlines. Review failure
  ordering explicitly if argument serialization moves into the outer write.
- Revalidate the phase-by-phase heap ledger in `limits.rs`, including simultaneous
  DOM, joined text, tool views, adapter scratch and final output. Keep nonblocking
  admission and reservation ownership until exchange and output consumers release.
  Fewer allocations alone do not prove a smaller peak or justify more admission.
- Extend actual `translate_unary` tests with complete output-byte comparisons,
  exact/over inner and outer limits, multiple tools, nested inputs and output
  failure after validated usage. Retain existing duplicate/escape, tool-only,
  adversarial-capacity and concurrent slow-output-owner regressions.
- Measure allocation calls, requested bytes, retained peak storage and CPU work
  separately for small and large tool inputs and mixed text/tool responses.
  Existing allocation probes exclude allocator overhead and transport/TLS; do not
  promote them to RSS or full-deployment memory proofs.

Original constraints: the repository HTTP body invariants,
`skills/01-architecture/02-gateway/00-overview.md`, and
`skills/04-review/ai-quota/regression-guards/anthropic-usage-observation-is-estimation-only.md`.
This task grants no general response aggregation, serializer replacement, SSE or
native-provider behavior change. Use IMPL after user selection. No runtime code
or new performance experiment is included in this task.
