---
name: real-ip-run-request-tracing-removed
description: Use when reviewing findings that `RealIp::run_request` should emit a `tracing::debug!` for the matched trusted-proxy group name; explains why the per-request debug event was removed and why "cold path" / "at most once per request" framing does not exempt it from Iron Rule 3.
---

# `RealIp::run_request` Trusted-Proxy-Match Tracing Is Removed (Fixed)

## Conclusion

`RealIp::run_request` (`edgion-gateway/src/plugins/http/real_ip/plugin.rs`) **must
not** emit `tracing::debug!` (or any `tracing::*`) for the matched trusted-proxy
group name. The `tracing::debug!(group = %group, ip = %client_ip,
"trusted_proxy_match")` call (with its surrounding `if let Some(group) =
extractor.matched_trusted_group(&client_ip)` block) was removed on 2026-05-26
because:

1. `run_request` is the `RequestFilter::run_request` impl, invoked once per
   request by `PluginRuntime::run_request_plugins` from the Pingora
   `request_filter` / `upstream_peer` data-plane phases — squarely the
   per-request hot path (`skills/03-coding/01-log-safety.md` Iron Rule 3).
2. The `if extractor.is_trusted_proxy(&client_ip)` branch that guarded the call
   fires for **every** request in the predominant production topology (gateway
   behind a load balancer / trusted proxy). At default `info` level it is a
   cheap per-request callsite-interest check; if an operator raises the gateway
   level to `debug` to troubleshoot — a routine action — it emits a formatted
   event per request, flooding logs and adding per-request latency/allocation
   under load.

The essential fact is already captured by `plugin_log.push("trusted
proxy:match")` (the sanctioned per-request access-log channel), so the
trusted-proxy match remains observable without `tracing`. `matched_trusted_group`
stays a `pub fn` on `RealIpExtractor` with its own unit test
(`matched_trusted_group_returns_name`), so removing its only production caller
does not create dead code.

## Decision Rule

When a review re-reports any of:

1. "`RealIp::run_request` should `tracing::debug!` the matched trusted-proxy
   group name / `trusted_proxy_match` for ops visibility"
2. "the call is a *cold path* — it fires at most once per request, only when the
   client IP itself is a trusted proxy — so Iron Rule 3 does not apply"
3. "downgrade to `tracing::trace!` / gate behind a debug-build cfg / route to a
   separate target instead of deleting"

→ close as **fixed + design intent**. Verify before closing:

- `real_ip/plugin.rs` `run_request` contains no `tracing::` calls (the `create`
  config-load path may keep its `tracing::error!` / `tracing::warn!` — those are
  ConfHandler-time, exempt).

Re-report **(2)** is **not accepted**: "at most once per request" is still
per-request, and the guarding branch is taken on essentially every request in
the LB/trusted-proxy topology. "Cold path" framing (including the now-removed
source comment that asserted it) does not exempt a `tracing::*` callsite on the
data-plane hot path. Re-report **(3)** is **not accepted** for the same reason
the DSL `Vm::execute` tracing was declined ([dsl-vm-execute-tracing-declined.md](dsl-vm-execute-tracing-declined.md)):
the prohibition is on the path class, not the level or target. If matched-group
visibility becomes a concrete ops need, append it to the PluginLog push
(`plugin_log.push_fmt`) or expose a bounded metric — promote that to a proper
task rather than re-opening this finding.

## Background Notes

- Decision date: 2026-05-26
- Source issue: `tasks/plugins/plugins-http-F001-tracing-debug-on-request-hot-path.md` (closed)
- Same class as gateway-routes-04 ([global-plugins-resolve-no-tracing.md](global-plugins-resolve-no-tracing.md))
  and gateway-routes-01 ([tcp-stream-plugin-no-tracing.md](tcp-stream-plugin-no-tracing.md)):
  a per-request/per-connection data-plane `tracing` call removed because the
  hot-path prohibition outranks the (otherwise access-log-available) signal.
- The prior Round B rationale that "the event is constructed / strings formatted
  before the level gate" was technically inaccurate (`tracing` gates on callsite
  interest before evaluating field values); the correct grounds are the Iron
  Rule 3 violation plus the live runtime cost when the level is raised to
  `debug`.
