---
knowledge_type: regression-guards
---

# Streaming usage is a monotonic cumulative snapshot

Provider usage objects in a stream are cumulative snapshots, not deltas.
OpenAI commonly emits only one final object, but compatible providers may emit
one on every chunk. First-wins records an early zero; blind last-wins permits a
lower or out-of-order frame to refund observed usage.

Merge repeated snapshots by component-wise high-water marks. Publish each
immutable multi-field result through `ArcSwapOption` so concurrent final
logging cannot combine fields from two updates. Cache detail remains optional
and is cleared if its merged components exceed total input tokens.

Quota tests must cross both producer shapes: semantic text/tool deltas add
bounded estimated units, while provider cumulative totals raise the same
request watermark monotonically. Worker tests separately lock down failed
incremental retry, final timeout, and cancellation on request drop.

## Single JSON document versus streaming snapshots

Within one non-streaming Chat or Responses JSON document, decoded duplicate selected keys
and replacement of whole `usage`/details parents use JSON last-wins semantics.
The bounded selective observer keeps values provisional until valid EOS;
malformed tails and incomplete documents cannot publish reported usage. This
single-document rule is distinct from merging successive provider snapshots.
After a complete document succeeds, publication uses the same shared snapshot
owner. Exact nonnegative u64 token counts and checked cache/total additions
prevent overflow from becoming an apparently valid smaller usage/cost result.

## Resolution references

- `test-05` — 2026-08-09.

## Shared AI JSON syntax whitespace

The shared offset-reporting parser `ai/protocols/json_span.rs::Parser::ws`
accepts only space, horizontal tab, line feed and carriage return, as specified
by [RFC 8259 section 2](https://www.rfc-editor.org/rfc/rfc8259.html#section-2).
This rule applies to both eager and offset-only parsing. Escaped `\f` inside a
JSON string remains valid, with the original quoted token offsets preserved.

ARCH-010 corrected the use of general ASCII whitespace, which includes form
feed. In the canonical Chat SSE path, `SseEventRef::semantic_data` retains that
valid UTF-8 byte and `observe_event` delegates to the shared parser. An invalid
JSON usage envelope now takes the existing default observation path and cannot
contribute reported counters or semantic fragments. Raw SSE bytes remain intact.
This is distinct from invalid UTF-8 poisoning an SSE event; the decoder's
malformed-event tolerance and quota termination gates remain unchanged.

Mirrored parser tests cover form feed before, within and after a document in
both modes, all four legal whitespace bytes, and escaped form-feed decoding and
offsets. The canonical SSE regression drives the real decoder and observer,
asserting empty usage/semantic observation, one unpoisoned event and exact raw
bytes. These tests define the acceptance boundary; fresh execution results belong
to the [implementation task](../../../../tasks/pending/architecture-json-span-whitespace/02-implementation.md).
This correction makes no quota-bypass, provider-conformance or complete strict
validation claim; offset-only unselected-string validation is outside its scope.
Re-evaluate if the whitespace helper or parsing-mode ownership changes.

- `ARCH-010` — shared JSON whitespace correction, 2026-10-12.

## Native Responses distinction

Responses advances the watermark only with reliable typed-terminal provider output totals.
Native delta/done/output repetitions do not add character estimates or duplicate charges.
The Chat semantic-delta estimator remains Chat-specific. Missing terminal usage or invalid
framing yields unknown actual billing; provisional admission estimates do not substitute for it.
Heartbeat/comment/id-only dataless blocks remain transparent, including under active Reject.
