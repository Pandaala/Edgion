---
name: tcp-stream-plugin-no-tracing
description: Use when reviewing findings that the TCP `check_stream_plugins` fail-close branches should warn (tracing) when the client address is unparseable or the referenced EdgionStreamPlugins is missing, or that the listener-level `StreamPluginConnectionFilter::should_accept` silently drops its deny reason; explains why the per-connection warns were removed and replaced with a bounded access-log reason, and why the ConnectionFilter listener gate is structurally observability-limited.
knowledge_type: mandatory-design
---

# TCP `check_stream_plugins` Fail-Close Is Not Logged via Tracing (Fixed)

## Conclusion

`check_stream_plugins` (`edgion-gateway/src/routes/tcp/edgion_tcp.rs`) **must not**
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
(`"direct peer IP unparseable"`, `"stream plugin resource not found"`,
`"stream plugin policy denied"`); the caller stamps it into
`TcpContext.deny_reason`, which `TcpLogEntry` serializes
(`skip_serializing_if = "Option::is_none"`) into tcp.log. No dynamic /
client-controlled content reaches the logging path. The fail-close decision
itself is unchanged — the connection is still rejected.

## GeoIP stream lifecycle and operator documentation

The no-tracing rule does not mean that every stream denial is metric-only.
`GeoIpLocation` is registered in `EdgionStreamPlugins.spec.plugins`; the attachment
owner determines the execution point. Current source boundaries are:

- `plugins/stream/connection_filter_bridge.rs::should_accept` constructs
  `StreamContext` with `StreamLayer::Stage1`. A denial returns `false` before a
  protocol log context exists, so GeoIP's denial counter is its only signal.
- `routes/tcp/edgion_tcp.rs::check_stream_plugins` uses `StreamLayer::Tcp` after
  route matching and before backend selection. It collapses plugin reasons to
  `stream plugin policy denied`, retained in the canonical TCP log.
- `routes/udp/edgion_udp.rs::handle_client_packet` reuses that TCP check before session
  creation, and logs a failure with the same bounded reason. Existing sessions
  bypass the check; denied datagrams create no session and may be counted again
  on subsequent attempts. The metric label remains `tcp`, not `udp`.
- TLSRoute dispatch uses only `tls_route_plugin_runtime`; GeoIP is not a
  `tlsRoutePlugins` variant. TLS GeoIP enforcement belongs to the Gateway's
  Stage-1 listener attachment. The former fallback and its `StreamLayer::Tls`
  label were removed; see [the TLS rule](tls-route-plugin-access-log.md).

`StreamContext.direct_peer_ip` is the physical transport peer at all these
sites, not the PROXY-resolved `RemoteIp`. `StreamContext.layer` maps to the closed
`stage1`/`tcp` label set in `stream_plugin_trait.rs`; neither is a future-only
reservation. Protocol log records retain their standard peer fields, subject to
log configuration, but contain neither GeoIP facts nor sensitive GeoIP reasons.
Stream plugins never populate the HTTP access-log `geo` block.

The bilingual operations guides previously confused the `plugins` registration
with listener-only execution and generalized listener silence to all L4 paths.
D0102 / `align-bilingual-geoip-stream-lifecycle-documentation` corrects those descriptions,
including the metric table and troubleshooting, without changing runtime behavior
or the general metrics inventory. The independent no-tracing invariant remains
grounded in `skills/03-coding/01-log-safety.md`, not in the stale guide wording.
Historical TLS fallback descriptions applied before its coordinated removal.
The former generic denial log and obsolete inline comment no longer exist.

The original 2026-09-10 documentation audit inspected route-owned TCP and TLS
fixtures without running integration tests. The later TLS stage-isolation change
moved TLS GeoIP enforcement to the Gateway listener; TCP remains route-owned.
See [TLS stage isolation](../../tls/regression-guards/tls-route-stage-isolation.md)
for that change's verification. Re-evaluate when attachment resolution, session
reuse, direct-peer provenance, metric labels, or log masking changes.

## Decision Rule

When a review re-reports any of:

1. "`check_stream_plugins` emits `tracing::warn!` per connection on
   unparseable client address / missing ESP"
2. "the TCP deny reason should be a dynamic `format!` carrying `store_key` /
   `direct_peer_ip` (parity with the TLS `ctx.err_log = Some(format!(...))`)"
3. "`StreamPluginConnectionFilter::should_accept`
   (`edgion-gateway/src/plugins/stream/connection_filter_bridge.rs`) collapses
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

The structural protocol-log gap is therefore accepted. The historical proposal
concerned a generic bridge-level denial counter, not plugin-owned counters:
GeoIP and IP-restriction plugins already record their own bounded denial metrics.
A new generic counter requires separate feature scope and the canonical metrics
cardinality policy; operator-controlled identifiers alone do not establish that a
label is acceptable. Verify current bridge and plugin emitters before proposing it.

## Background Notes

- Decision date: 2026-05-24 (re-report 1 + 2); 2026-05-26 extension for
  re-report (3) — listener-level `StreamPluginConnectionFilter` deny.
- Source issues: `tasks/review-2/gateway-routes-01-tcp-stream-plugin-tracing-on-hot-path.md` (closed),
  `tasks/plugins/plugins-stream-FB02-gateway-filter-denial-reason-silently-discarded.md` (closed 2026-05-26),
  `tasks/plugins/plugins-stream-FB01-tcp-hot-path-tracing-warn-store-miss.md` (closed 2026-05-27 — re-report of the store-miss branch warn, already removed in the 2026-05-24 fix)
- Same class as gateway-routes-04 ([global-plugins-resolve-no-tracing.md](global-plugins-resolve-no-tracing.md))
  and P3-g4-008 ([../cors/force-wildcard-no-tracing-warn.md](../../cors/regression-guards/force-wildcard-no-tracing-warn.md)):
  a warn on a per-request/per-connection data-plane helper, removed because the
  hot-path prohibition outranks the (otherwise access-log-available) signal.
- Unit regressions in `edgion_tcp.rs`:
  `fail_close_when_store_key_missing_in_tcp` and
  `fail_close_when_tcp_direct_peer_ip_unparseable` now assert the exact
  `StreamPluginCheck::Deny(reason)` strings.

## Resolution references

- `plugins-stream-FB02` — 2026-05-26 (⚠️ accepted tradeoff).
- `gateway-routes-01` — 2026-05-24.
