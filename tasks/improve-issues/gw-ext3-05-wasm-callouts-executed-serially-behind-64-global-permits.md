# gw-ext3-05: Wasm `proxy_http_call` callouts run one at a time per request behind a 64-permit process-wide gate

- **Category**: best-practice
- **Severity**: P2 (worth doing)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/wasm/callout.rs:31` (`MAX_CONCURRENT_CALLOUTS = 64`), `callout.rs:91-149` (`resolve_callouts_on`: `while let Some(callout) = pending.next()` awaits each callout before starting the next), `callout.rs:176-213` (`execute_callout_with_client_and_permits`: permit wait counted inside the callout timeout), `edgion-gateway/src/plugins/http/wasm/host_abi.rs:47,1305` (`MAX_CALLOUTS_PER_REQUEST = 8`), `edgion-gateway/src/plugins/http/wasm/plugin.rs:277-300` (request path holds the VM lease while callouts resolve)

## Summary
Proxy-Wasm guests commonly dispatch several HTTP callouts from one hook (for example an authz check
plus a policy/feature lookup) and pause until all answers arrive. Envoy starts every dispatched call
immediately and delivers responses as they complete, so the pause lasts `max(latency)`. Edgion
queues the calls and executes them strictly sequentially, so the pause lasts `sum(latency)` (up to 8
calls x `calloutTimeoutMs`). Each call additionally waits for one of only 64 process-wide permits,
and the wait consumes that call's own timeout budget. During the whole sequence the request pins an
exclusive VM instance (see `gw-ext-01`).

## Current behavior (evidence)
- `proxy_dispatch_http_call` only queues a `PendingCallout` (`host_abi.rs`, `host_http_call`).
- `resolve_callouts_on` takes the queue and runs
  `execute_callout_with_client_and_permits(...).await` inside the loop, then delivers that one
  response via `call_on_http_call_response` before starting the next request (`callout.rs:107-139`).
  There is no `join`/`FuturesUnordered`; independent calls never overlap.
- Every callout acquires one permit from a global `Semaphore::new(64)` (`callout.rs:31,198`), shared
  by all Wasm plugins, all requests and tick tasks. The acquisition happens inside
  `timeout_at(deadline, attempt)`, so under contention callouts fail as timeouts and the guest sees
  the proxy-wasm "failed call" (zero headers).
- With callouts of 50 ms, the whole gateway is capped at ~1,280 callouts/s regardless of core count
  or upstream capacity; a request dispatching three 100 ms calls waits >= 300 ms instead of ~100 ms.

## Industry comparison
- Envoy `proxy_http_call` / `dispatch_http_call` sends through the cluster's async client
  immediately; multiple outstanding calls per context run concurrently and are bounded by the
  target cluster's circuit breakers (`max_requests`, `max_pending_requests`), not by a fixed
  process-wide semaphore.
- Edgion's own ext_authz-like plugins and AI fan-out paths already issue outbound requests
  concurrently through `build_outbound_client`, bounded per destination.

## Impact
- Latency of paused guests grows linearly with the number of callouts; the same filter behaves
  differently than under Envoy/Istio, which matters because Edgion advertises Proxy-Wasm SDK
  compatibility.
- The 64-permit gate turns a busy gateway into spurious callout failures; with `failOpen: false`
  these surface as 500s, with `failOpen: true` as silently skipped authorization logic.
- Longer pauses extend VM pinning, compounding the per-request VM ceiling in `gw-ext-01`.

## Suggested direction
1. Start all callouts queued by a hook concurrently (`FuturesUnordered`), bounded by
   `MAX_CALLOUTS_PER_REQUEST`, and deliver each `proxy_on_http_call_response` serially on the VM in
   completion order (the guest is single-threaded, delivery stays sequential; only network I/O
   overlaps). Keep the round cap and request deadline.
2. Replace the single process-wide 64 permit gate with per-origin limits (the allowlist already
   enumerates origins) sized like cluster circuit breakers, and acquire the permit before starting
   the per-call timeout or report "overflow" distinctly from "timeout".

## Notes
- Checked `skills/04-review/wasm/regression-guards/request-execution-lifecycle.md` and
  `tick-quiescence-on-zero-reference-eviction.md` (mentions "serial callouts" only as a tick
  eviction duration bound). Neither records serial execution as an accepted Proxy-Wasm deviation.
- Related: `gw-ext-01` (exclusive VM per request), `gw-ext-02` (ExtProc's own 64-stream cap).

## Verification (independent)

Verdict: CONFIRMED. Severity P2 kept.
- `resolve_callouts_on` (`wasm/callout.rs:91-149`) awaits each `execute_callout_with_client_and_permits` inside the `while let` before starting the next; no concurrent join.
- Global `Semaphore::new(64)` (`callout.rs:29-35`); the permit is acquired inside `timeout_at(deadline, ...)` (`:198-213`), so contention surfaces as a failed callout.
- Note for the fix: the loop also abandons remaining callouts once the guest resumes after a response; concurrent dispatch must preserve that abandonment semantics (cancel outstanding futures).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `wasm/callout.rs`, `host_abi.rs` and `plugin.rs` are unchanged; `MAX_CONCURRENT_CALLOUTS = 64` is still at `callout.rs:31`, and the callout loop is still serial.
- No new or changed `skills/04-review` page in this area accepts the behavior; the only area review change is a test-process-isolation note in `wasm/regression-guards/request-execution-lifecycle.md`, which does not affect production behavior.
