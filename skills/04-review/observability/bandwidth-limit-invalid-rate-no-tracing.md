---
name: bandwidth-limit-invalid-rate-no-tracing
description: Decision rule for findings claiming `BandwidthLimit::create` should emit `tracing::warn!`/`tracing::error!` when the configured `rate` string fails to parse. Read this before re-reporting either "doc claims a log but none exists" or "add a warn so misconfiguration is observable".
---

# `BandwidthLimit::create` invalid-rate path — NO RUNTIME LOG, DOC CLARIFIED

## Conclusion

`BandwidthLimit::create` (`edgion-gateway/src/plugins/http/bandwidth_limit/plugin.rs`) deliberately
does NOT emit `tracing::warn!` / `tracing::error!` on the `parse_rate().unwrap_or(0)` fallback.
Fix suggestions of the form

- "add `tracing::warn!` so a misconfigured `rate` is visible in log aggregation"
- "the doc comment promises a log — make the code match by emitting one"
- "wrap `unwrap_or` in `unwrap_or_else(|| { tracing::warn!(...); 0 })`"

are **not accepted**. The doc comment was the bug; the runtime behaviour is correct. The
comment was rewritten (2026-05-27, plugins-http-FB02) to describe the actual behaviour and
to explain why no log is needed.

## Core Rationale

**1. The production path can never reach the `unwrap_or(0)` branch.**
`PluginRuntime::add_from_upstream_response_body_filters` (`plugin_runtime.rs:634`) calls
`entry.plugin.validation_error()` before construction. `BandwidthLimitConfig::get_validation_error`
returns `"invalid rate format, expected: '1mb', '512kb', '1024', etc."` whenever
`parse_bandwidth_rate` returns `None`. That error is recorded into `invalid_plugins`,
`seal_fail_closed` promotes a non-empty `invalid_plugins` into a fail-closed `500` for the route
and surfaces a K8s status condition on the owning resource. An operator who misconfigures
`rate: "1 mb"` therefore sees:

- The route returning `500` (not silently unlimited throughput) — strong signal.
- A K8s status condition naming the invalid plugin and the parse error — actionable signal.

A `tracing::warn!` from `BandwidthLimit::create` would be **redundant and weaker** than these
two existing signals.

**2. The only caller that ever hits the sentinel is the unit test.**
`tests::test_bandwidth_limit_invalid_rate` builds `BandwidthLimit` directly from
`BandwidthLimitConfig { rate: "invalid", .. }` without going through the runtime validation
gate. The `unwrap_or(0)` sentinel exists so this synthetic test path produces a deterministic
no-op rather than panicking, not because the production path is expected to reach it.

**3. The proposed `tracing::warn!` placement is on a plugin-construction call site.**
`BandwidthLimit::create` is invoked from `PluginRuntime` at config load (startup or hot
reload), so a `tracing::warn!` there would technically satisfy
[`observability-review-rule.md`](observability-review-rule.md) Iron Rule 3 exception 1
("Configuration load events in ConfHandler / conf_handler_impl"). The rejection is not
hot-path-based — it is value-based: an unreachable branch does not deserve an instrumentation
hook.

## Fix Suggestions Not Accepted

- Add `tracing::warn!(rate = %config.rate, "BandwidthLimit: invalid rate string, throttling disabled")` in `BandwidthLimit::create` — duplicates the `invalid_plugins`/`seal_fail_closed` signal, instruments an unreachable branch in production.
- Promote the sentinel to a `Result` and propagate the parse error through `create_upstream_response_body_filter_from_edgion` — the runtime validation gate already does this earlier and more cleanly.
- Remove the `unwrap_or(0)` sentinel and panic instead — would crash the unit test path; the test legitimately exercises the "what does the plugin do if a caller skips validation" contract.

## Re-evaluation Triggers

Re-open the decision only if **all** of the following change:

1. The runtime validation gate is removed or bypassed for `BandwidthLimit` (so the sentinel
   becomes reachable in production).
2. `seal_fail_closed` stops translating `invalid_plugins` into a fail-closed `500` + K8s status
   condition for upstream-response-body-filter plugins.
3. A production incident shows operators failing to notice a `BandwidthLimitConfig` validation
   error because no `tracing::warn!` was emitted.

## Reference

- Original finding: `plugins-http-FB02` (closed 2026-05-27 via doc-comment fix).
- Related design record: [`../rate-limit/bandwidth-limit-per-stream.md`](../rate-limit/bandwidth-limit-per-stream.md) (per-stream scope decision; the same plugin).
- Runtime validation gate: `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` `add_from_upstream_response_body_filters` → `entry.plugin.validation_error()`.
