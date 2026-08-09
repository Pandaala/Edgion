# Response-body pipeline — async hook, terminate, bounded emit, idle bound

| Key | Value |
|-----|-------|
| Created | 2026-08-04 |
| Status | todo — the largest of the three, and the one that unlocks the use case |
| Where the change goes | Pingora fork (`pingora-proxy/src/proxy_trait.rs` + its four call sites, `pingora-core` for the idle bound), then Edgion plugins |
| Effort | S–M for the signature change; M for the sink |
| Source of the numbers | verified 2026-08-04 against Envoy protos and Traefik/nginx primary docs — see §4 |

## Why this exists

This is the capability the whole request-body seam work kept pointing at: **inspect, reassemble and act on a response body while it streams** — SSE event reassembly, streaming audit, token counting, mid-stream rejection. None of it is possible today.

**Correct the record first, because two earlier statements in this project were wrong.** It is *not* true that "there is no response-body filter of any kind". Pingora already has `response_body_filter` (`proxy_trait.rs:601`), `upstream_response_body_filter` (:580), `upstream_response_trailer_filter` (:591) and an **async** `response_trailer_filter` (:619); the module layer adds two more. **Edgion already uses one** (`upstream_response_body_filter`, for bandwidth throttling).

The real gaps are narrower, and that makes this much cheaper than the archived AI-0a design assumed:

1. the hook is **sync** — `fn response_body_filter(..) -> Result<Option<Duration>>`;
2. it is **1-in-1-out** — a filter cannot emit more (or fewer) chunks than it received;
3. there is **no clean terminate** — rejecting means returning `Err`, which on a committed response yields truncation plus a closed connection, not a graceful end;
4. its only back-pressure is a `Duration` the pump sleeps on.

**The load-bearing discovery:** all four call sites — `proxy_h1.rs:877`, `proxy_h2.rs:1256`, `proxy_custom.rs:643`, `proxy_cache.rs:444` — are **already inside an `async fn` that already `.await`s** (the cache site awaits `time::sleep(duration)` literally two lines below the call). So item 1 is **a signature change, not a new pipeline**. The archived AI-0a design proposed a whole new hook plus sink; its hard-won analysis (the P1–P4 post-conditions, the synthesized `HttpTask::Done`, the twelve contract clauses) is still valuable — but it applies to **the sink**, not to a new hook that no longer needs building.

## What must ship together, and why

These four are not independent items. Shipping a subset produces a defect rather than a partial feature:

- **async + terminate alone** → a filter can inspect but not reassemble-and-emit; the headline use case (SSE) stays out of reach.
- **the 1→N emit sink without a bound** → OOM. The bound must be per batch, not per input chunk (AI-0a measured the difference: a per-chunk cap multiplies by the pump's 4-task batch).
- **the sink without auto-stripping `content-length`** → a length-changing filter puts a protocol violation on the wire.
- **terminate implemented as "write a local reply"** → wrong by construction once the response is committed. Traefik proved this publicly: its `maxResponseBodyBytes` overflow is documented to return 500 but issue #10687 shows it **trims the body and keeps the 200**. You cannot promise a status change after commit. **Terminate must therefore be a clean truncation** (finish the downstream body), not a local reply. nginx's equivalent escape hatch, `return ngx.ERROR` from a body filter, is documented to produce *invalid* responses — do not copy it either.

Add to the same release:

- **a response-body inter-chunk idle bound.** Once a filter is allowed to hold response bytes, an unbounded hold is a new way to pin a stream. Our `write_timeout` bounds a single flush, not the gap between chunks.

## The numbers, with sources

| Setting | Value | Why |
|---|---|---|
| Response-body idle timeout, default | **5 minutes** | Envoy `stream_idle_timeout` default (HCM proto, field 24). Its wording is exactly our semantics: "each time an **encode/decode** event for headers **or data** is processed, the timer will be reset" — idle, both directions, covers inter-chunk response gaps. |
| Per-route override | **mandatory, not optional** | Envoy ships a **15 s `RouteAction.timeout`** on by default — but that is a **total** bound. The lesson is not the number; it is that anything this aggressive must be overridable per route. |
| Idle, never total | — | Traefik's `writeTimeout` **is** a total bound ("from the end of the request header read to the end of the response write") — which is precisely why Traefik ships it at **0 / disabled**. A total response bound is unshippable for streaming. nginx's 60 s `send_timeout` is idle, so it does not argue for a lower number. |

## What this buys over the comparators

nginx **forbids cosockets in its body filter**, so it cannot do an async audit mid-body at all. Making our hook async does not put us level with nginx — it puts us past it.

## Do not copy

- Envoy `FULL_DUPLEX_STREAMED` — still WIP, forces trailer `SEND`, and carries a separate mutation API.
- nginx `return ngx.ERROR` from a body filter (documented to emit invalid responses) and its manual `content-length` clearing.
- Traefik's buffer-everything-to-enable-retry — the wrong trade for LLM traffic; our per-request `request_retry_allowed` predicate is strictly better.
- Disk-spill buffering (nginx, Traefik) — wrong trade here, and a large amount of machinery.

## Verification bar

The seam suite's rules apply (`tasks/todo/important_test/00-how-to-dispatch.md`). Specifically for this work:

- a filter that changes body length must be pinned end-to-end on **both** H1 and H2 downstreams, asserting the wire framing (no `content-length` alongside a changed body, chunked where required);
- the emit bound must have a test that trips it, asserting what the client receives rather than that an internal counter moved;
- terminate-as-truncation must be asserted on a **committed** response — the exact case Traefik gets wrong;
- the idle bound needs both directions: a stream idle past the bound is released, **and** a stream producing a chunk just inside the bound is never released (a bound that fires on a progressing stream is worse than none).
