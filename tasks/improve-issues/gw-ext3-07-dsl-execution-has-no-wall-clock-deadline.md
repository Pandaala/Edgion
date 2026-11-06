# gw-ext3-07: DSL execution has no wall-clock deadline; external calls are bounded only per call, not by the request deadline

- **Category**: best-practice
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/dsl/lang/vm.rs:376-...` (`Vm::execute`: step/call/loop budgets only), `vm.rs:1199` (`tick_call` per builtin), `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs:1485-1670` (`call_builtin_http`: `timeout_ms = min(script value, httpMaxTimeoutMs)`, line 1563; `.timeout(...)`, line 1633), `builtins.rs:1820` (`redis.rate_limit` per-call `policy.timeout`), other `redis.*` arms (pool command deadline only), `edgion-gateway/src/plugins/http/dsl/plugin.rs:169-176` (response stage runs the same VM under `block_in_place` + `block_on`), contrast `edgion-gateway/src/plugins/http/wasm/plugin.rs:279,814-824` (Wasm callouts are clamped to the route/gRPC request deadline)

## Summary
The DSL sandbox bounds dispatches (`maxSteps`), builtin calls (`maxCallCount`, default 500, max
100,000), and per-loop iterations, but not elapsed time. Every `http.call` gets its own timeout of up
to `httpMaxTimeoutMs` (default 5 s) and every Redis builtin its own pool deadline, and none of them is
clamped to the time remaining in the request. A script that loops over a list and calls a dependency
per element can hold a request for `iterations x timeout` — e.g. 100 x 5 s = 500 s with defaults —
when that dependency degrades. The Wasm plugin, by contrast, already derives an absolute deadline
from `timeouts.request` / `grpc-timeout` and clamps callouts to it.

## Current behavior (evidence)
- `Vm::execute` checks `tick_step` per opcode and `tick_call` per builtin; there is no `Instant`
  or deadline in `VmState` (`vm.rs`).
- `call_builtin_http` computes `timeout_ms` from the optional script argument and
  `runtime.max_timeout_ms` only, then `builder.timeout(Duration::from_millis(timeout_ms))`
  (`builtins.rs:1563,1633`). Nothing consults `session.ctx()` start time or route timeout.
- `redis.*` arms await the LinkSys client directly; only `redis.rate_limit` wraps a
  `tokio::time::timeout(policy.timeout, ...)` (`builtins.rs:1820`), again per call.
- `skills/01-architecture/02-gateway/dsl-engine-security-model.md` ("Strict JSON reconstruction and
  nontruncating Redis GET") states "there is no general VM execution timeout" as a fact, without a
  rationale.
- In the response stage the whole execution runs inside `block_in_place` + `block_on`
  (`dsl/plugin.rs:169`), so the unbounded wall-clock time also occupies a Tokio worker (`gw-ext-04`).
- The route deadline is only re-checked at upstream peer selection (`gw-core-04`); a request-stage
  DSL that runs past it still finishes all its calls before the 504 is produced, and a response-stage
  DSL is never cut off.
- Wasm (`wasm/plugin.rs:814-824`, `request_deadline`) clamps every callout to
  `min(route deadline, gRPC client deadline)` and poisons the VM when it expires — the inconsistency
  is within the same plugin family.

## Industry comparison
- Envoy ext_authz / ext_proc enforce a per-message timeout *and* the stream is subject to the route
  timeout and `max_stream_duration`; Envoy Lua `httpCall` timeouts are per call but the stream timer
  still terminates the request.
- Kong/OpenResty plugins run under `ngx` request timeouts (`proxy_read_timeout`, `lua_socket_*`)
  and commonly use a request-level deadline helper; APISIX's `ext-plugin` uses one RPC timeout per
  phase, not per call.

## Impact
- A dependency slowdown multiplies into request hold times of minutes, pinning downstream
  connections and, in the response stage, Tokio workers.
- Operators reasoning from `timeouts.request` assume an end-to-end bound that DSL plugins silently
  exceed.

## Suggested direction
Add an execution deadline to `VmState`: `min(now + maxExecutionMs, request deadline)` where the
request deadline is computed exactly like Wasm's `request_deadline`. Clamp every `http.call` and
Redis call to the remaining time (`timeout = min(per-call, remaining)`), and fail with a dedicated
`RuntimeError::DeadlineExceeded` (handled by the existing error policy) once it is reached. A single
optional `maxExecutionMs` knob (default derived from the route timeout) is sufficient; no new
failure-policy shape is needed.

## Notes
- Checked `skills/04-review/dsl/regression-guards/response-stage-http-timeout.md` (verifies that the
  per-call timeout is honoured in the response stage; it does not consider aggregate time) and the
  DSL security model page cited above. No task tracks an execution deadline.
- Related: `gw-ext-04` (response stage blocks a worker), `gw-core-04` (route timeout is not a
  wall-clock deadline).

## Verification (independent)

Verdict: CONFIRMED, DOWNGRADED P2 -> P3.
- `call_builtin_http` clamps `timeout_ms` only to `runtime.max_timeout_ms` (`dsl/lang/builtins.rs:1563-1566`) and applies it per request (`:1633`); `vm.rs` contains no `Instant`/deadline.
- `dsl-engine-security-model.md:389` states there is no general VM execution timeout; Wasm callouts are clamped to the request deadline.
- Downgrade reason: the multiplied hold time requires an operator-authored script that issues many sequential dependency calls; a single call is bounded (default 5 s). The broader "plugin phases are not bounded by the route deadline" issue is cross-cutting and overlaps `gw-core-04`; the DSL-specific clamp is a cheap consistency fix.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- `dsl/lang/vm.rs`, `builtins.rs` and `dsl/plugin.rs` are unchanged; the VM still has only step, call and loop budgets.
- Updated docs and `dsl-engine-security-model.md` still state that "there is no general VM execution timeout" (fred -> redis-rs wording only).
