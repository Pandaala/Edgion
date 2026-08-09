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
