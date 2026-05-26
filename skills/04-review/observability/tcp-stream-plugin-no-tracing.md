---
name: tcp-stream-plugin-no-tracing
description: Use when reviewing findings that the TCP `check_stream_plugins` fail-close branches should warn (tracing) when the client address is unparseable or the referenced EdgionStreamPlugins is missing, or that the listener-level `StreamPluginConnectionFilter::should_accept` silently drops its deny reason; explains why the per-connection warns were removed and replaced with a bounded access-log reason, and why the ConnectionFilter listener gate is structurally observability-limited.
---

# TCP `check_stream_plugins` Fail-Close Is Not Logged via Tracing (Fixed)

## Conclusion

`check_stream_plugins` (`src/core/gateway/routes/tcp/edgion_tcp.rs`) **must not**
emit `tracing::warn!` (or any `tracing::*`) on its fail-close branches. The two
warns were removed on 2026-05-24 because:

1. `check_stream_plugins` is called from `EdgionTcpProxy::handle_connection` once
   per accepted TCP connection — a data-plane connection hot path
   (`skills/03-coding/01-log-safety.md` Iron Rule 3 "TCP/TLS connection path"
   entry). A single deleted / unsynced `EdgionStreamPlugins` (ESP) made the
   warn fire on *every* new connection until the resource was restored — log
   volume scaling with the TCP connection rate.
2. The TLS counterpart `run_tls_route_plugins` (`routes/tls/mod.rs`) already
   used the correct data-plane channel (`ctx.err_log`); the TCP path diverged
   by writing the same diagnostic to tracing, so TLS fail-close lands in
   tls.log while TCP fail-close went to tracing — cross-protocol inconsistency
   that makes ops debugging harder.

The fix mirrors the TLS pattern but keeps the reason **bounded**: the
`StreamPluginCheck::Deny` variant now carries a `&'static str` reason
(`"client address unparseable"`, `"stream plugin resource not found"`,
`"stream plugin policy denied"`); the caller stamps it into
`TcpContext.deny_reason`, which `TcpLogEntry` serializes
(`skip_serializing_if = "Option::is_none"`) into tcp.log. No dynamic /
client-controlled content reaches the logging path. The fail-close decision
itself is unchanged — the connection is still rejected.

## Decision Rule

When a review re-reports any of:

1. "`check_stream_plugins` emits `tracing::warn!` per connection on
   unparseable client address / missing ESP"
2. "the TCP deny reason should be a dynamic `format!` carrying `store_key` /
   `client_addr` (parity with the TLS `ctx.err_log = Some(format!(...))`)"
3. "`StreamPluginConnectionFilter::should_accept`
   (`src/core/gateway/plugins/stream/connection_filter_bridge.rs`) collapses
   `StreamPluginResult::Deny(reason)` to `false` with no `tracing` / counter /
   access-log field — operators cannot tell which listener-level deny fired or
   why; thread the reason into a context object / `TcpLogEntry`-equivalent /
   `tracing` call"

→ close as **fixed + design intent**. Verify before closing:

- `edgion_tcp.rs` `check_stream_plugins` contains no `tracing::` calls.
- `StreamPluginCheck::Deny(&'static str)` and `TcpContext.deny_reason` /
  `TcpLogEntry.deny_reason` are present.
- `connection_filter_bridge.rs` `should_accept` still returns plain `bool`
  with no `tracing::*` call (the listener-level deny remains silent by
  design — see (3) below).

Re-report **(2)** is **not accepted**: the TLS path uses a dynamic `err_log`
String for masked detail, but the TCP path deliberately uses a bounded
`&'static str` (no client-controlled bytes on the logging path, no per-
connection `format!` allocation on the hot path). The bounded reason already
classifies the failure; verbose detail is not worth the allocation / leakage
surface.

Re-report **(3)** is **not accepted**: Pingora's `ConnectionFilter` trait
exposes only `async fn should_accept(&self, addr: Option<&SocketAddr>) -> bool`
— no context object, no log buffer, no return-side metadata channel. The
three previously-proposed fixes are all closed:

- *Emit `tracing::info!/warn!` per denied connection* — violates Iron rule 3
  (`skills/03-coding/01-log-safety.md` "TCP/TLS connection path"), which is
  exactly the prohibition that motivated removing the original
  `check_stream_plugins` warns (gateway-routes-01). The listener gate runs
  *earlier* than `check_stream_plugins`, so it has even tighter hot-path
  exposure: any unauthenticated peer can drive the warn rate.
- *Thread the reason through a new ctx-like object* — the Pingora trait
  signature is fixed at the dependency boundary; we cannot widen it without
  forking pingora-core. The connections that reach this filter have not yet
  produced a `TcpContext`, so there is no downstream log entry to mutate
  either.
- *Add an access-log entry on listener-level deny* — by construction the
  `TcpLogEntry` lifecycle starts inside `EdgionTcpProxy::process_new`, which
  is only reached *after* `ConnectionFilter::should_accept` admits the
  connection. Forcing a log emission from the filter would require building a
  parallel "rejected-connection log" sink, which is a feature, not a fix, and
  would itself be subject to the per-connection hot-path constraint.

The structural observability gap is therefore accepted. Operators that need
counted listener-level denies should request a metric counter as a separate
feature (Prometheus counter increment is hot-path safe and bounded-cardinality
— `store_key` is operator-controlled, low cardinality), tracked as
`tasks/todo/` work rather than a review-finding fix. Verify before re-
reporting that no such counter has landed yet.

## Background Notes

- Decision date: 2026-05-24 (re-report 1 + 2); 2026-05-26 extension for
  re-report (3) — listener-level `StreamPluginConnectionFilter` deny.
- Source issues: `tasks/review-2/gateway-routes-01-tcp-stream-plugin-tracing-on-hot-path.md` (closed),
  `tasks/plugins/plugins-stream-FB02-gateway-filter-denial-reason-silently-discarded.md` (closed 2026-05-26)
- Same class as gateway-routes-04 ([global-plugins-resolve-no-tracing.md](global-plugins-resolve-no-tracing.md))
  and P3-g4-008 ([../cors/force-wildcard-no-tracing-warn.md](../cors/force-wildcard-no-tracing-warn.md)):
  a warn on a per-request/per-connection data-plane helper, removed because the
  hot-path prohibition outranks the (otherwise access-log-available) signal.
- Unit regressions in `edgion_tcp.rs`:
  `fail_close_when_store_key_missing_in_tcp` and
  `fail_close_when_tcp_client_addr_unparseable` now assert the exact
  `StreamPluginCheck::Deny(reason)` strings.
