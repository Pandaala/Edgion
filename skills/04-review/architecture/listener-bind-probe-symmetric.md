---
name: listener-bind-probe-symmetric
description: Use when reviewing findings of the form "`add_http_listener` / `add_tcp_listener` / `add_tls_terminate_to_tcp_listener` / `add_tls_passthrough_listener` defer bind to `run_forever`; an EADDRINUSE / EACCES at startup surfaces as `.expect(\"Failed to build listeners\")` panic with no listener name; asymmetric with UDP". The synchronous TCP bind probe was added in runtime-F009 to mirror the UDP path.
---

# Listener Bind Probe — Symmetric With UDP (HTTP/HTTPS/TCP/TLS)

**Fix date**: 2026-05-28
**Linked issue**: runtime F-009 (Low / BadLogic) — closed ✅ with code fix.

## Conclusion

`src/core/gateway/runtime/server/listener_builder.rs` performs a synchronous
`std::net::TcpListener::bind(&addr)` probe before every Pingora call that
queues a TCP / TLS listener address. The probe socket is dropped immediately;
Pingora re-binds with `SO_REUSEADDR` when `start_service` runs. Failures
return an `anyhow::Error` enriched with `gateway` / `listener` / `addr`
context, matching the existing UDP path.

Probe call sites (all in `listener_builder.rs`):

- `add_http_listener` — HTTP → HTTPS redirect `add_tcp`
- `add_http_listener` — HTTPS `add_tls_with_settings`
- `add_http_listener` — HTTP `add_tcp`
- `add_tcp_listener` — `Listeners::tcp`
- `add_tls_terminate_to_tcp_listener` — `add_tls_with_settings`
- `add_tls_passthrough_listener` — `Listeners::tcp`

The helper is `fn probe_tcp_bind(addr: &str, gateway_key: &str, listener_name: &str) -> Result<()>`.

## Core Rationale

Pingora's `add_tcp` / `add_tls_with_settings` / `Listeners::tcp` only queue a
`ServerAddress`; the actual `bind()` + `listen()` is deferred to
`Listeners::build()` inside `ServiceTrait::start_service`
(`pingora-core-0.8.0/src/services/listening.rs:270-280`), and a failure there
hits `.expect("Failed to build listeners")` in a Pingora-spawned tokio task.
The panic message carries no gateway / listener name, all services abort
together, and the operator has no structured signal to correlate.

The UDP path already binds synchronously in
`add_udp_listener` (`std::net::UdpSocket::bind(&addr)?`) and returns a
context-carrying `Err`. The probe makes the four other protocols behave the
same way.

The TOCTOU window between the probe drop and Pingora's real bind is bounded:
`configure_listeners` (`runtime/server/base.rs`) runs single-threaded before
`run_forever`, the `bound_listeners` HashSet deduplicates the same
`ListenerKey` inside one configuration pass, and `SO_REUSEADDR` is set by
Pingora. The realistic conflict cases (another process already holding the
port, missing `CAP_NET_BIND_SERVICE` for ports <1024) are all detected
synchronously by the probe.

## Fix Suggestions Not Accepted

- "Set a custom panic hook around `run_forever` to enrich
  `Failed to build listeners`" — the panic still aborts all listeners
  together, the hook can't return a Result to the caller, and a stringly
  parsed message is more fragile than synchronous binding.
- "Add an opt-out flag so operators can disable the probe" — the probe
  cost is one syscall pair at startup; there is no use case to disable it.
- "Probe and then keep the socket open / pass it to Pingora" — Pingora's
  listener API does not accept pre-bound `TcpListener` handles; the probe
  drop is required.

## Re-evaluation Triggers

- Pingora upstream exposes a `try_build_listeners` returning `Result`, in
  which case the probe becomes redundant.
- A new transport (QUIC, etc.) is added to `add_listener` and bypasses the
  probe.

## Reference Cases

- Issue file (closed): `tasks/runtime/runtime-F009-listener-builder-bind-error-not-fail-fast.md`
- UDP synchronous bind precedent: `add_udp_listener` in the same file.
