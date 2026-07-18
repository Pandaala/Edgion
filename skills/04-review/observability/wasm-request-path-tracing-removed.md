---
name: wasm-request-path-tracing-removed
description: Use when reviewing Wasm plugin hot-path findings that report tracing on request/response/body paths or guest-controlled data (proxy_log message, callout URL/origin) reaching tracing; covers the Iron-rule-1/3 removal.
status: fixed
finding_id: runtime-06
closed: 2026-07-18
---

# Wasm request-path tracing (guest message + callout URL) removed — FIXED

## Conclusion

`WasmPlugin`'s request/response/per-chunk-body hooks and the guest-reachable
host imports (`proxy_log`, `proxy_dispatch_http_call`, `execute_callout`) emitted
`tracing::{debug,warn}` on the data-plane hot path, several of them copying
**guest-controlled or request-derived** data into the event — the `proxy_log`
full `%msg` (Iron rule 1) and the callout `%url`/`%origin`/`%method` (a query
string may carry credentials). Every such site was removed; the pre-existing
`record_wasm_*` metrics and `PluginLog` fail-mode entries already carry the
bounded signal, so observability does not regress. Re-reports of the form "add a
`tracing::debug!`/`warn!` back for the trap / acquire-fail / callout-fail /
guest log / body-Pause" are **not accepted**.

## Core Rationale

**1. Iron rule 3 is literal — any level, including `debug!`, is forbidden on the
request/connection hot path.** The removed sites all sit inside
`run_request` / `run_leased_request` / `run_leased_response` /
`run_upstream_response_filter` / `run_upstream_response_body_filter` / `apply_ops`
(all reached per request or per body chunk), and inside `host_http_call` /
`execute_callout` (reached synchronously from the guest during
`call_on_request_headers`). See `skills/03-coding/01-log-safety.md` Iron rule 3.

**2. Two of the sinks were also Iron-rule-1 secret leaks.** `proxy_log` wrote the
full guest message via `tracing::debug!(target:"wasm_guest", %msg, …)`; a guest
can feed a request-derived `authorization` value straight in. `execute_callout`
logged `%url = origin + guest path` (query may carry `?token=…`). Debug level does
not make raw secrets acceptable.

**3. The signal was already covered without tracing.** Fail modes route through
`fail_mode(log, reason)` → `PluginLog` + `record_wasm_fail_mode`; callout
outcomes through `record_wasm_callout(label, "denied"|"over_limit"|"error"|"ok")`;
the body-Pause incompatibility through `record_wasm_body_pause_unsupported`. The
guest log lines stay retained in `HostState.log_lines` (bounded, reset per phase)
and are reported only as a **count** (`logs=N`) by `finish_phase` — matching the
finding's "bounded in-memory diagnostic count only" suggestion.

**4. The once-per-module body-Pause WARN was removed with its latch.** It was
rate-latched (≤1 per module) and carried no request data, but it still originated
in the per-chunk body callback and the body filter has no `PluginLog` channel.
Rather than keep a `// tracing-allow` exception, the WARN and the now-dead
`ModuleEntry::latch_body_pause_warning` / `body_pause_warned` latch were deleted;
the unconditional `record_wasm_body_pause_unsupported` metric is the sole channel.

## Intentionally retained (NOT part of this finding)

These are cold / control-plane / background sites and legitimately keep tracing:

- `plugin.rs` `WasmPlugin::build` `calloutAllowlist` drop WARN — construction path
  (the no-tracing gate's `COLD_FN` also exempts `build`).
- `callout.rs:69` egress-guarded client build error — one-time `OnceLock` init.
- `vm_manager.rs` preload / load / eviction / tick / OCI logs — background tick
  task and cold module load/cache management, outside request processing.

## Fix Suggestions Not Accepted

- Re-add any `tracing::*` (even `debug!`, even a separate `target:`) on the wasm
  request/response/body hooks or the guest-reachable host imports — Iron rule 3.
- Log the guest `proxy_log` message anywhere on the request path — Iron rule 1.
- Log the callout `%url`/`%origin`/`%method` — guest-controlled, may carry creds.
- Restore the once-per-module body-Pause WARN behind `// tracing-allow` — the
  bounded metric already carries it and the latch is gone.

## Re-evaluation Triggers

Re-open only if:

- A dedicated, opt-in, rate/size-limited and sensitivity-aware guest-log *channel*
  (not tracing) is designed — that is a new feature via `tasks/todo/`, not a
  review fix.
- The `UpstreamResponseBodyFilter` trait gains a `PluginLog` (or ctx) channel,
  making a bounded per-module body-Pause signal expressible off-tracing.

## Known gate coverage gap

`cicd/checks/check_gateway_no_tracing.py` only covers `plugins/http/*/plugin.rs`
via `DP_GLOBS`; the wasm module's `host_abi.rs` and `callout.rs` are **not** in
its scan set, so their hot-path tracing was invisible to the gate and had to be
found by review. `plugin.rs` is covered (and is now clean). If a future change
adds tracing to `host_abi.rs`/`callout.rs` request-reachable functions, the gate
will not catch it — audit by inspection.

## Reference Cases

- `skills/03-coding/01-log-safety.md` Iron rules 1 & 3.
- Sibling data-plane tracing removals: [real-ip-run-request-tracing-removed.md](real-ip-run-request-tracing-removed.md),
  [tcp-stream-plugin-no-tracing.md](tcp-stream-plugin-no-tracing.md),
  [dsl-vm-execute-tracing-declined.md](dsl-vm-execute-tracing-declined.md).
- Files: `edgion-gateway/src/plugins/http/wasm/{plugin.rs,host_abi.rs,callout.rs,vm_manager.rs}`.
