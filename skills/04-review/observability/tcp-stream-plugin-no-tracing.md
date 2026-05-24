---
name: tcp-stream-plugin-no-tracing
description: Use when reviewing findings that the TCP `check_stream_plugins` fail-close branches should warn (tracing) when the client address is unparseable or the referenced EdgionStreamPlugins is missing; explains why the per-connection warns were removed and replaced with a bounded access-log reason.
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

→ close as **fixed + design intent**. Verify before closing:

- `edgion_tcp.rs` `check_stream_plugins` contains no `tracing::` calls.
- `StreamPluginCheck::Deny(&'static str)` and `TcpContext.deny_reason` /
  `TcpLogEntry.deny_reason` are present.

Re-report **(2)** is **not accepted**: the TLS path uses a dynamic `err_log`
String for masked detail, but the TCP path deliberately uses a bounded
`&'static str` (no client-controlled bytes on the logging path, no per-
connection `format!` allocation on the hot path). The bounded reason already
classifies the failure; verbose detail is not worth the allocation / leakage
surface.

## Background Notes

- Decision date: 2026-05-24
- Source issue: `tasks/review-2/gateway-routes-01-tcp-stream-plugin-tracing-on-hot-path.md` (closed)
- Same class as gateway-routes-04 ([global-plugins-resolve-no-tracing.md](global-plugins-resolve-no-tracing.md))
  and P3-g4-008 ([../cors/force-wildcard-no-tracing-warn.md](../cors/force-wildcard-no-tracing-warn.md)):
  a warn on a per-request/per-connection data-plane helper, removed because the
  hot-path prohibition outranks the (otherwise access-log-available) signal.
- Unit regressions in `edgion_tcp.rs`:
  `fail_close_when_store_key_missing_in_tcp` and
  `fail_close_when_tcp_client_addr_unparseable` now assert the exact
  `StreamPluginCheck::Deny(reason)` strings.
