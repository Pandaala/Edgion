# gw-ai3-07: Every AI request pins 2-3 full copies of its request body (plus a spill file) in the shared body budget for the entire streaming response

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/http/proxy.rs:337` (`session.read_request_body(None)` → `BodyStorageView::read_all`)
  - `edgion-gateway/src/request_body/storage.rs:667-686` (`read_all`: disk source materialized into `materialized: OnceCell<Bytes>` held by the view inner for the life of `BodyStorage`)
  - `edgion-gateway/src/request_body/storage.rs:1867-1925` (`BodyStorage::write`: bodies above `memoryLimit` (default 128 KiB) are spilled to a temp file first)
  - `edgion-gateway/src/ai/http/proxy.rs:912-926` (`canonical_openai_request`: `stream_options` injection produces a new, budget-untracked `Bytes`)
  - `edgion-gateway/src/ai/runtime/attempt_plan.rs:88` (`ReadyAiAttemptPlan.request_body: Bytes`, kept in ctx until request end)
  - `edgion-gateway/src/ai/runtime/dispatch.rs:461-517` (model rewrite / conversion → `ctx.overwrite_request_body(body)` → another budgeted copy)
  - `edgion-gateway/src/request_body/storage.rs:1179-1191` (`overwrite_body`: `MemoryBuffer::copy`)
  - `edgion-gateway/src/request_body/budget.rs:18-27` (process-wide defaults: 256 MiB memory, 1024 captures)
  - `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs:82` (`release_view` drops only the plugin view, not the plan or the storage inner)

## Summary

AI requests are long-lived (seconds to minutes of streaming) and large (long-context prompts are
hundreds of KiB to several MiB). The current flow keeps, until the downstream response finishes:
(1) the spill file written during capture, (2) the full materialized body cached by `read_all`,
(3) the plan's own `request_body` (an *untracked* heap copy whenever usage tracking is injected),
and (4) a second budgeted copy published by `overwrite_request_body` whenever the provider model
differs from the requested name (alias mapping) or a conversion runs. The copies in (2) and (4) are
charged to the process-wide request-body memory budget (default 256 MiB) shared with every other
body-capturing plugin, and none of them is needed once the final response head is committed
(no retry or failover is possible after that).

## Current behavior (evidence)

1. Capture: `BodyStorage::write` keeps ≤128 KiB in memory, otherwise creates a temp file and writes
   every subsequent chunk through the global 32-slot storage I/O lane (`storage.rs:1891-1925`).
2. AiProxy reads the whole body (`proxy.rs:337`); for a disk source `read_all` allocates a
   `MemoryBuffer` of the full length (budgeted) and caches it in `inner.materialized`
   (`storage.rs:676-684`). `BodyStorageViewInner` is co-owned by the Pingora `BodyStorage`, so this
   cache lives until the session ends, regardless of `release_view`.
3. Chat streaming with `inject_usage_tracking`: `request::prepare` returns a patched `Bytes`
   (`BytesMut` → `freeze`, not a `MemoryBuffer`), stored as `plan.request_body` — an extra full
   copy outside the budget (`proxy.rs:912-926`).
4. Dispatch: if `provider_model != logical_model` (`rewrite_model`) or a conversion runs, the new
   body is published with `overwrite_request_body`, which copies it into another budgeted
   `MemoryBuffer` (`dispatch.rs:504-516`, `storage.rs:1185`). The plan itself is restored into ctx
   (`dispatch.rs:240`) and is never dropped before the request ends.

Worked example: a 200k-token coding-agent prompt ≈ 800 KB of JSON, routed via an alias:
materialized 800 KB + overwrite 800 KB = 1.6 MB of budget (plus an untracked 800 KB plan copy and
an 800 KB spill file) for the full duration of the stream. 256 MiB / 1.6 MB ≈ 160 concurrent
long-context streams per gateway process; the next body-capturing request of *any* kind (AI or
not) receives `CapacityExceeded` → 503 `BodyCaptureCapacityExceeded`.

## Industry comparison

- Envoy buffers a request body for retries only until the upstream response headers arrive
  (`retry` + `buffer` filter); the buffer is released once retry is no longer possible, and the
  overload manager accounts it against a per-connection limit.
- NGINX keeps a single body representation (memory *or* temp file, `client_body_in_single_buffer`
  when the consumer needs it contiguous) and does not re-materialize it.

## Impact

Long-context AI concurrency per gateway is capped by request-body copies rather than by network or
CPU, and the cap fails closed for unrelated plugins sharing the budget. The disk round-trip (write
then read back) also adds latency to every large AI request.

## Suggested direction

1. Choose one representation for AI requests: when a consumer requires the full contiguous body
   (AiProxy, AiGuard), read it from the in-memory capture without spilling, or have AiProxy declare
   a contiguous requirement up front so capture skips the spill (bounded by the same budget).
2. Make the plan hold the *published* snapshot (budgeted `MemoryBuffer`) instead of a separate
   untracked `Bytes`; publish the injected/rewritten body once and reuse it for all attempts that
   target the same provider model.
3. Release replay state (plan body, `materialized` cache, spill file) once the final response head
   is committed and no further attempt is possible.

## Notes

- `skills/04-review/http-body/regression-guards/request-body-aggregate-budget.md` documents the
  budget mechanics (and that storage copies opaque inputs). It does not discuss AI request lifetime
  or the multiple concurrent representations; this is a new consequence, not a re-litigation of the
  budget design.
- arch-02 mentions the budget as an existing overload knob; it does not cover this retention.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `request_body/storage.rs:667-686`: `read_all` caches a disk-source body in `inner.materialized` (a `OnceCell` on the shared inner), which lives as long as `BodyStorage`. `request-body-aggregate-budget.md:37` confirms that materialization/cache results are charged.
- `ai/runtime/dispatch.rs:504-516` calls `ctx.overwrite_request_body` when the body changed. That is `MemoryBuffer::copy` into a second budgeted buffer (`storage.rs:1179-1191`).
- `ai/http/proxy.rs:915-929` stores the injected `stream_options` body as an owned `Bytes` in the plan, outside the budget.
- The default process budget is 256 MiB (`request_body/budget.rs:18-27`). No review page addresses releasing AI replay copies after the final response head. Copies only multiply for bodies above the 128 KiB memory limit, which long-context traffic exceeds.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `request_body/storage.rs` is unchanged: `read_all` (`:667`) still caches into `inner.materialized: OnceCell<Bytes>` (`:582`, `:680`), and `overwrite_body` (`:1179`) still copies into a new budgeted buffer. `budget.rs` only switched `fetch_update`→`try_update`.
- `ai/http/proxy.rs:337` still reads the full body and `canonical_openai_request` (`:912-926`) still stores the injected body as an unbudgeted `Bytes`; `dispatch.rs:506` still calls `ctx.overwrite_request_body` on rewrite/conversion.
- No new code releases the AI replay copies after the final response head.
