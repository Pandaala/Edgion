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

## Native Responses distinction

Responses advances the watermark only with reliable typed-terminal provider output totals.
Native delta/done/output repetitions do not add character estimates or duplicate charges.
The Chat semantic-delta estimator remains Chat-specific. Missing terminal usage or invalid
framing yields unknown actual billing; provisional admission estimates do not substitute for it.
Heartbeat/comment/id-only dataless blocks remain transparent, including under active Reject.
