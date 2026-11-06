# gw-ai3-05: Adaptive-routing telemetry silently drops a process-phase-dependent share (0-100%) of every second's samples, and resets on any backend edit

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/ai/routing/adaptive.rs:127-150` (`SecondBucket::enter`: write accepted only if the slot's epoch already equals the current second)
  - `edgion-gateway/src/ai/routing/adaptive.rs:248-288` (`tick`: the current second's slot is rotated only at the *end* of the tick, line 287)
  - `edgion-gateway/src/ai/routing/adaptive.rs:479-492` (`run_adaptive_telemetry_ticker`: `interval(1s)` anchored at task start, `MissedTickBehavior::Skip`)
  - `edgion-gateway/src/ai/routing/adaptive.rs:221-238` (`record_success_ttft`, `record_charged_tokens` use `current_unix_second()`)
  - `edgion-gateway/src/ai/routing/candidate.rs:10, 185-232` (`LATENCY_MATURE_SAMPLES = 8` in 30 s; `charged_tokens_60s` score)
  - `edgion-gateway/src/ai/runtime/backend_store.rs:317-333, 356-372, 383` (`changed_ai_backend_targets` → `remove_target` on any `RuntimeAiBackend` change)
  - `edgion-gateway/src/ai/http/proxy.rs:859-899` (`capture_adaptive_states`: key = request logical model string)

## Summary

The per-second ring only accepts a sample if its slot was already rotated to the current Unix
second, and the slot is rotated by a 1 s `tokio::time::interval` whose phase is fixed by when the
process started — not aligned to second boundaries. Every sample recorded between the start of
second `S` and the tick that rotates slot `S` is discarded. With tick phase `p ∈ [0,1)`, the gateway
loses a fraction `p` of all LowestLatency TTFT samples and LowestUsage charged tokens, for the life
of the process. `p` is uniformly random per replica (expected 50%, up to ~100%), so replicas see
different maturity and usage. Separately, any change to an EdgionBackend (secret rotation, pricing,
an added alias) discards all adaptive state for that backend, and telemetry keys fragment across
model aliases and never exist for wildcard-catalog models.

## Current behavior (evidence)

- `enter(second)` returns `false` unless `self.second == second` (`adaptive.rs:128, 144`).
- `tick(now_second)` publishes the window and only then calls
  `self.buckets.bucket(now_second).rotate_to(now_second)` (`adaptive.rs:284-287`; the comment
  acknowledges "Events arriving before this cold tick are dropped").
- The ticker is `interval(Duration::from_secs(1))` started at an arbitrary instant
  (`adaptive.rs:480`). If it fires at `S + 0.9`, records during `[S, S+0.9)` hit a slot whose epoch is
  still `S-61` (closed) and are dropped: 90% loss, every second.
- Consequences in selection (`candidate.rs`): with 90% loss, a backend needs ~80 successful requests
  in 30 s (instead of 8) to become "mature" for LowestLatency, so low-traffic routes stay in the
  exploration path indefinitely; `charged_tokens_60s` for LowestUsage is scaled by `(1-p)` with
  extra noise from tick jitter. Two replicas with different `p` make different decisions on
  identical traffic.
- Reset on edit: `replace_all`/`update` compute `changed_ai_backend_targets` by `Arc::ptr_eq`, then
  `adaptive_store.remove_target(target)` (`backend_store.rs:327-333, 366-372`). Any non-equal
  recompilation — e.g. a rotated API-key Secret or a pricing tweak — drops all histories for that
  backend across every route.
- Key fragmentation: `capture_adaptive_states` keys telemetry by the requested logical model string;
  the cold path pre-creates keys only for `backend.models.keys()` (exact names and aliases,
  `adaptive.rs:665-670`). Requests for `gpt-4o` and its alias `gpt4o` accumulate separately;
  wildcard-catalog models (`resolve_model` via `wildcard_models`) get no state at all and are
  always treated as "no telemetry".

## Industry comparison

- Envoy's least-request / Peak-EWMA (and Edgion's own `lb/selection/p2c.rs`) update state on the
  request path with no epoch gating, so no sample is lost.
- Time-bucketed rings in Prometheus client libraries / HdrHistogram rotate *lazily on write*
  (the writer that observes a newer epoch rotates the slot) or rotate ahead of the boundary, never
  "after the second has started".

## Impact

LowestLatency and LowestUsage behave differently per replica and much more noisily than designed;
low-volume routes may never leave exploration; credential rotation or catalog edits wipe learned
state. Combined with gw-ai-02 (argmin herding), adaptive modes are hard to reason about in
production.

## Suggested direction

1. Pre-rotate: have the tick rotate slot `now_second + 1` (and grow the ring to
   `USAGE_WINDOW_SECONDS + 2` so the published 60 s window is never overwritten), or let `enter`
   perform a lock-free CAS rotation when it observes a stale closed epoch for the current second.
2. Keep adaptive state across backend recompiles unless the endpoint/provider identity changed
   (credential secret values and pricing do not change latency or token velocity).
3. Key telemetry by the resolved canonical model (`ResolvedModel.canonical_name`) rather than the
   requested alias, so aliases share a series and wildcard models resolve to a bounded catalog key.

## Notes

- `skills/04-review/cpu-memory/regression-guards/adaptive-telemetry-reconcile-is-incremental.md`
  covers key reconciliation, not sample loss; it does not discuss the tick phase or reset-on-edit.
- gw-ai-02 covers the selection algorithm (argmin over stale telemetry); this finding is about the
  telemetry being incomplete and randomly scaled, which persists even if gw-ai-02 is fixed.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `ai/routing/adaptive.rs:127-150`: `enter(second)` refuses writes unless the slot epoch already equals `second`. `tick` rotates the current slot only after publishing (`:284-287`), and the comment admits "Events arriving before this cold tick are dropped".
- `run_adaptive_telemetry_ticker` (`:479-492`) uses `tokio::time::interval(1s)` anchored at task start, not at second boundaries. The dropped share therefore equals the tick phase within each second and differs per replica.
- `ai/runtime/backend_store.rs:317-333,383-396`: any `!Arc::ptr_eq` backend change calls `adaptive_store.remove_target`, so any recompile resets history.
- `adaptive-telemetry-reconcile-is-incremental.md` covers key reconciliation only.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `ai/routing/` is unchanged between feature-08-30 and HEAD: `adaptive.rs:127-150` `enter()` still refuses writes to an unrotated slot, and `tick` still rotates the current slot only after publishing (`:284-287`); the ticker (`:479-492`) is still a phase-unaligned `interval(1s)`.
- `ai/runtime/backend_store.rs` diff only removes `count_ai_backends`; `remove_target` on any changed backend remains at `:330` / `:369` (`changed_ai_backend_targets`, `:383`).
- `capture_adaptive_states` (`http/proxy.rs:859`) still keys by the requested model string.
