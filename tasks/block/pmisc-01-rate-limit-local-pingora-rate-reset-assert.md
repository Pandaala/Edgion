# pmisc-01: RateLimitLocal panics on concurrent requests during the first second of a sub-second `Rate` (Pingora `maybe_reset` assert underflow)

- **Category**: reachable-panic
- **Severity**: Medium
- **Scope**: cross-module
- **Trigger source**: timing
- **Location**:
  - `/Volumes/ExtStore/ws3/pingora/pingora-limits/src/rate.rs:174` (Edgion fork `Pandaala/pingora` branch `edgion_v0.9.1`, `Rate::maybe_reset`: `assert!(new >= now - 1000)` on the lost-CAS branch; `now - 1000` wraps when `now < 1000` in release, and overflows in debug)
  - `/Volumes/ExtStore/ws3/pingora/pingora-limits/src/rate.rs:145-147` (`maybe_reset`: unchecked `now - last_reset` when another thread already published a newer `last_reset`; wraps in release, panics in debug)
  - `edgion-gateway/src/plugins/http/rate_limit_local/plugin.rs:110-114` (`RateLimitLocal::get_rate`: lazily builds `Rate` with the configured interval, so `Rate::start` is the first request of each plugin generation)
  - `edgion-gateway/src/plugins/http/rate_limit_local/plugin.rs:248` (`run_request`: `rate.observe(&limit_key, 1)` on every request)

## Summary
`pingora_limits::rate::Rate::maybe_reset` measures `now` in milliseconds since the `Rate` was created.
When two request threads both decide a window reset is due and race on `last_reset_time.compare_exchange`,
the loser executes `assert!(new >= now - 1000)`. If the loser's `now` is below 1000 ms, `now - 1000`
wraps to about `u64::MAX` in release (overflow checks are off), so the assertion fails and panics. A reset
can only be due before the first second when the configured `RateLimitLocal.interval` is below `1s`
(for example `100ms`, `500ms`, or even `0ms`, which the GEP-2257 grammar and `check_fields` accept). Any
client that sends concurrent requests to such a route in the first second of a plugin generation can
trigger the panic. The same panic can also fire after the first second if the winning thread stalls for
more than 1 s between reading the clock and its CAS. That case is unlikely and would need CPU starvation.

## Reachability
- **Entry point**: `RequestFilter::run_request` for `RateLimitLocal` in `edgion-gateway/src/plugins/http/rate_limit_local/plugin.rs`
- **Data flow**: downstream request → `run_request` → `resolve_rate_limit_key` → `get_rate()` (first call creates `Rate` with `start = Instant::now()`) → `Rate::observe` → `maybe_reset` → `past_ms >= reset_interval_ms` on two threads with the same `last_reset` → one CAS fails → `assert!(new >= now - 1000)` with `now < 1000` → panic
- **Preconditions**: an `EdgionPlugins` `RateLimitLocal` entry with `interval` below `1s` attached to a route; the Gateway runs a multi-threaded runtime (Pingora worker threads); requests arrive concurrently during the first 1000 ms after the plugin instance's first request. A fresh window opens on every config generation that rebuilds the plugin and on every Gateway restart. With `interval >= 1s` the underflow cannot happen at a lost CAS (then `now >= interval >= 1000`), except for the starvation case above.
- **Minimal input**: route with `RateLimitLocal { rate: 1000, interval: "100ms", key: [ClientIP] }`. Send one request to start the `Rate`, then 8+ parallel requests repeatedly between t≈100 ms and t≈1000 ms. Each 100 ms boundary is a race window, and nine such windows fall within the first second. With `interval: "0ms"`, every concurrent pair in the first second races.

## Reproduction sketch
Executed in a scratch crate outside the repository, at
`/private/tmp/claude-501/-Volumes-ExtStore-ws3-Edgion/6857449d-fb84-4de2-b0f3-9c1e3ff4fae5/scratchpad/pmisc_rate_repro`.
It depends on the same local fork path as the workspace `[patch]`, `/Volumes/ExtStore/ws3/pingora/pingora-limits`,
and was built with `cargo run --release`. The workload ran 8 OS threads calling `Rate::observe(&"k", 1)` in
a loop for the first 1000 ms of a fresh `Rate::new_with_estimator_config(interval, 4, 1024)`, for 5 trials
per interval:

```
interval=0ms    panicked_threads=30
interval=100ms  panicked_threads=35
interval=500ms  panicked_threads=4
interval=1000ms panicked_threads=0
panic message: rate.rs:174:17: assertion failed: new >= now - 1000
```

Gateway-level equivalent (not executed): configure the route above, then run
`for i in $(seq 200); do curl -s -o /dev/null http://gw/route & done` immediately after the first request.
Some connections are reset instead of receiving 200 or 429.

## Impact
The panic unwinds out of the plugin future inside the Pingora per-connection task. The workspace has no
`panic = "abort"`, so tokio catches it at the task boundary. The affected downstream connection or request
is dropped without a response, and the worker thread and process survive. The losing request belongs to
any client, not only the attacker, so legitimate concurrent requests on the same route fail during the
window. `Rate` state remains usable because the winner's CAS already succeeded. Exposure is limited to the
first second of each plugin generation, or rare scheduler stalls. If the gateway is ever built with
`overflow-checks = true` or in debug, `now - 1000` and `now - last_reset` panic directly whenever reached.

## Suggested fix
Fix it in the Edgion Pingora fork (`pingora-limits/src/rate.rs`). On the lost-CAS branch, replace the
assertion with a non-panicking check such as `debug_assert!(new >= now.saturating_sub(1000))`, or drop it.
Compute `past_ms` as `now.saturating_sub(last_reset)` so a thread that read the clock before another
thread's publication does not wrap and wipe both windows. If the fork cannot change promptly, add a
defensive Edgion-side guard: reject `RateLimitLocal.interval` below `1s`, or at least `0ms`, in
`RateLimitLocalConfig::check_fields`. A zero interval is semantically meaningless anyway. This shrinks but
does not eliminate the starvation case. Add a regression test in the fork (`pingora-limits` unit tests,
multi-threaded first-second race with a 100 ms interval). If the config guard is added, also test it in
`edgion-resources/tests/unit/resources/edgion_plugins/plugin_configs/rate_limit_local_test.rs`.

## Notes
- No `skills/04-review/rate-limit/` page covers `pingora_limits::rate::Rate` reset semantics. A page under
  `skills/04-review/rate-limit/regression-guards/` should be added once fixed.
- A second, non-panicking release-mode defect exists on the same path: `now - last_reset` wraps when
  `last_reset > now` (stale clock read), and the reset branch then CASes `last_reset_time` backwards and
  clears both windows. That silently zeroes counts, a rate-limit under-count, but it is not a panic.
- `RateLimitLocal` is the only consumer of `pingora_limits::rate::Rate` in `edgion-gateway/src` outside tests.
- Confidence: high. The panic was reproduced against the exact fork source in release mode. Reaching it
  through the Gateway also needs the sub-second interval configuration and concurrency inside the
  first-second window, which is why the severity is Medium.

## Status: deferred, waiting for upstream (2026-11-12)

- Upstream already tracks this defect. Issue
  [cloudflare/pingora#1104](https://github.com/cloudflare/pingora/issues/1104) (opened 2026-11-03)
  reports both the `now - last_reset` race wrap and the first-second `now - 1000` assertion
  underflow. PR [#1105](https://github.com/cloudflare/pingora/pull/1105) (commit `a7b3167`)
  switches both subtractions to `saturating_sub` and adds a multi-threaded regression test.
  Both were open with no maintainer response on 2026-11-12, and `upstream/main` `3f87e92`
  (2026-11-09) still carried the unchanged code (present since the 2023-05-08 initial commit).
- Decision: no Edgion or fork code change for now; adopt the upstream fix normally once it lands.
- An Edgion-side `interval >= 1s` floor in `RateLimitLocalConfig::check_fields` was prototyped and
  reverted. It removes the sub-second trigger, but independent review found it does not fully
  close the panic. With the default `interval: "1s"`, at the first window boundary
  (`now == 1000`), threads that read the clock at 999 ms but load `last_reset` after another
  thread published 1000 see `999 - 1000` wrap and attempt a backwards CAS. The loser among them
  then evaluates `999 - 1000` in the assertion and panics. That needs at least 3 requests within
  nanoseconds of that boundary in each plugin generation. Raising the floor above 1s is not viable
  because `1s` is the default.
- Revisit trigger: #1105 (or an equivalent fix) merges upstream, or the user decides to
  cherry-pick it into the fork earlier. After adopting it, add the fork regression test, consider
  rejecting `interval: 0` separately (it disables limiting; tracked in
  `tasks/improve-issues/gw-plugin-04-rate-limit-local-fixed-window-and-sub-second-interval-hazards.md`),
  and record the resolution under `skills/04-review/rate-limit/`.
