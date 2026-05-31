---
name: filter-iteration-control-sync-phase-tradeoff
description: Use when reviewing plugin-chain "no iteration control" findings (no Envoy-style StopIteration / continueDecoding / async pause-resume; sync response phase forces block_in_place; thread starvation under spike; no stop-and-buffer in the response phase); covers why the sync UpstreamResponseFilter / UpstreamResponseBodyFilter contract is an accepted Pingora-bound tradeoff and why an async response phase is a feature, not a narrow fix.
---

# Plugin Chain Has No Envoy-Style Iteration Control (StopIteration / async pause-resume) — DESIGN-TRADEOFF (accepted)

## Conclusion

Edgion's plugin phases are split into async phases (`RequestFilter`, `UpstreamResponse`, both
`async fn` Pingora callbacks) and sync phases (`UpstreamResponseFilter`,
`UpstreamResponseBodyFilter`, which map to Pingora's sync `upstream_response_filter` /
`upstream_response_body_filter` callbacks). The sync phases have no "stop the chain, do async
work, resume" mechanism (Envoy's `FilterHeadersStatus::StopIteration` + `continueDecoding/
continueEncoding`). A plugin needing async I/O in the sync response-header phase must bridge via
`tokio::task::block_in_place` (as the shipped `DslPlugin::run_upstream_response_filter` does,
`src/core/gateway/plugins/http/dsl/plugin.rs`). This is an **accepted tradeoff** bounded by
Pingora's sync callback contract.

Fix suggestions of the form "add a per-plugin `block_in_place` timeout", "introduce an
`AsyncUpstreamResponseFilter` phase as a narrow fix", or "give the response phase Envoy-style
stop-and-buffer / response replacement" are **not accepted** as review-issue fixes — the first
is already effectively provided, the second/third are language-level features that belong in
`tasks/todo/` via the brainstorming workflow.

## Core Rationale

**1. The sync response-phase contract comes from Pingora, not from Edgion**

`UpstreamResponseFilter` / `UpstreamResponseBodyFilter` are deliberately sync because the
underlying Pingora hooks (`upstream_response_filter` returning `Result<()>`,
`upstream_response_body_filter` returning `Result<Option<Duration>>`) are sync. There is no
Edgion-level lever to make them pausable; a true non-blocking pause-resume would require an async
variant of these callbacks upstreamed into Pingora. This is the same root that
[concurrency/block-in-place-multi-thread-runtime.md](../concurrency/block-in-place-multi-thread-runtime.md)
established for the narrower `block_in_place`/`spawn_blocking` framing — F008 covers the
"current_thread panic / spawn_blocking doesn't compile" angle; this entry covers the broader
"thread starvation under load / no async response phase / no stop-and-buffer" framing.

**2. The thread-parking window is bounded, not indefinite**

The "N concurrent requests park N Pingora workers" concern is real but self-limiting: the only
shipped plugin that bridges sync→async in the response phase is `DslPlugin`, and a single
`block_in_place` call is time-bounded by the DSL VM step budget plus the `http.call` request
timeout and `max_response_body_bytes` cap. There is no path to indefinite parking. The
"short-term: add a hard per-plugin `block_in_place` timeout" mitigation from the finding is
therefore already provided by the VM step budget + builtin timeouts — a second timeout layer
would be redundant. Industry L7 gateways that build on a sync upstream-response hook accept the
same bridge; the degradation under extreme spike is a bounded latency rise on the shared worker
pool, not data corruption or a crash.

**3. An async response phase is a feature, not a narrow fix**

The finding's medium-term direction — an `AsyncUpstreamResponseFilter` phase that runs in
Pingora's async `response_filter` (phase 7) but is handed the phase-5 upstream response headers —
is a genuinely good idea, but it is a new phase in the plugin-phase abstraction: it touches the
phase model, plugin registration, the runtime dispatch loop, and every plugin trait that would
opt in. That is cross-module work with real regression surface, exactly what the review-issue
workflow's scope gate is meant to push into `tasks/todo/` for the brainstorming workflow. It is
not a minimal, low-risk fix.

**4. The response-replacement gap is tracked separately**

Point 3 of the finding (a sync-phase `ErrResponse` can only be logged via
`check_response_stage_terminate` — `plugin_runtime.rs:1190` — and cannot write an alternate body
to the client) is the subject of the sibling finding
`filter-04-response-filter-cannot-replace-response` and is handled there, not here.

## Fix Suggestions Not Accepted

- Add a hard per-plugin `block_in_place` timeout — already provided by the DSL VM step budget +
  `http.call` request timeout + `max_response_body_bytes`; a second layer is redundant and adds
  hot-path cost.
- Introduce an `AsyncUpstreamResponseFilter` phase as a review-issue fix — it is a new
  plugin-phase feature (phase model + registration + dispatch + traits), promote to `tasks/todo/`
  and design via brainstorming instead.
- Give the sync response phase Envoy-style stop-and-buffer / `continueEncoding` / response
  replacement — requires either an async Pingora response callback (out of project control,
  upstream-only) or buffering the full response in the sync context (no `await` available); both
  are out of scope for a narrow fix. The response-replacement subset is tracked by `filter-04`.
- Switch `block_in_place` to `spawn_blocking` or add a runtime-flavor panic guard — already
  rejected by F008 (non-`'static`/non-`Send` borrows; production is always multi-thread).

## Re-evaluation Triggers

Re-open this decision only if:

- A real production incident shows a `block_in_place`-based response-phase plugin causing worker
  starvation under load that the VM step budget / builtin timeouts did not bound.
- Pingora ships an async variant of `upstream_response_filter` (then true non-blocking iteration
  control becomes implementable and the bridge can be removed).
- A second shipped plugin needs heavy async I/O in the sync response phase, multiplying the
  bridge's blast radius beyond DslPlugin — at which point the `AsyncUpstreamResponseFilter` phase
  should be promoted to a task.

## Reference Cases

- Finding `filter-02-no-iteration-control` (`tasks/review-envoy/`), 2026-05-29 — ⚠️ accepted
  tradeoff; closed without code change.
- Narrower sibling rule:
  [concurrency/block-in-place-multi-thread-runtime.md](../concurrency/block-in-place-multi-thread-runtime.md).
- Response-replacement subset: `filter-04-response-filter-cannot-replace-response` (same review set).
- Source: `src/core/gateway/plugins/http/dsl/plugin.rs` (`run_upstream_response_filter`);
  `src/core/gateway/plugins/runtime/plugin_runtime.rs:1190` (`check_response_stage_terminate`);
  `src/core/gateway/plugins/runtime/traits/upstream_response_filter.rs` (sync trait contract).
