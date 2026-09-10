---
name: udproute-stream-plugins-annotation-wiring
description: Use when reviewing UDPRoute stream-plugin annotation compilation, dynamic store lookup at session creation, and the limits of policy hot reload for established sessions (udp-F012 / F-012).
knowledge_type: regression-guards
---

# UDPRoute `edgion.io/edgion-stream-plugins` annotation wiring (udp-F012)

## Conclusion and scope

UDPRoute supports `edgion.io/edgion-stream-plugins` through a compiled route-unit
key and dynamic plugin-store lookup before new-session backend selection. Only
the Stage-1 connection-context runtime (`spec.plugins`) applies; UDP has no TLS
handshake and does not execute `tlsRoutePlugins`.

The current owner is `UdpRouteUnit::from_route` in
`edgion-gateway/src/routes/udp/route_unit.rs`. It calls the shared
`resolve_stream_plugin_store_key` with the route metadata and stores the result
on `UdpRouteUnit::stream_plugin_store_key`. `GlobalUdpRouteManagers::initialize_route`
delegates to this constructor during configuration updates. The schema in
`edgion-resources/src/resources/udp_route.rs` has neither a
`stream_plugin_store_key` nor a `stream_plugin_runtime` field; `UDPRouteRule`
contains only `backend_refs`. Gateway runtime state does not belong on that schema.
See [UDP route schema Arc sharing](udp-route-schema-arc-sharing.md).

## Enforcement and hot-reload boundary

`EdgionUdpProxy::handle_client_packet` in
`edgion-gateway/src/routes/udp/edgion_udp.rs` passes the selected unit's key to
the shared `check_stream_plugins` in `routes/tcp/edgion_tcp.rs`, before backend
selection and `get_or_create_session`. No annotation (or a whitespace-only
annotation) produces `None`, which returns `Allow` immediately. A present key
is looked up in the current `StreamPluginStore` snapshot; a missing resource or
policy denial fails closed. An empty Stage-1 runtime is a no-op pass-through.
The route unit stores a key, not a snapshot of the compiled plugin runtime.

Enforcement is per session admission, not per packet. The established-session
fast path returns before route/plugin evaluation, reusing the pinned upstream;
a denied new-session attempt never creates a session. The
[F-016 session fast-path guard](../../cpu-memory/regression-guards/udp-established-session-route-pipeline-per-packet.md)
owns that optimization and its session-affinity boundary.

A policy hot reload is observed by subsequent new-session checks, not by
already-established sessions. `SESSION_TIMEOUT` is a 60-second **inactivity
threshold**, not a maximum session lifetime or a policy-revocation deadline.
Both `record_upstream_send_result` and `record_downstream_send_result` refresh
`last_activity` on successful sends. Cleanup runs after a 10-second sleep per
loop and selects sessions whose inactivity exceeds the threshold; it is not an
exact expiry timer. Continuing successful traffic can keep an existing session
active beyond 60 seconds without re-running plugins. There is no fixed
60-second bound for a hot-reload-to-deny to affect such a session.

This page does not introduce immediate invalidation, a maximum session lifetime,
or per-packet policy checks. It also does not establish correctness of session
retirement races or shutdown draining. The corresponding backend-authorization
boundary is documented in
[Denied L4 Backend Refs Fail Closed](l4-backend-ref-denied-fail-closed.md).

Gateway-level attachment is a separate lifecycle. `add_udp_listener` creates an
independent Tokio task rather than a Pingora `Service`, so it never installs the
listener `ConnectionFilter` built by `apply_connection_filter`. Consequently a
Gateway annotation does not protect its UDP listener; UDP policy must be attached
to `UDPRoute`. This does not contradict reuse of the Stage-1 `spec.plugins`
runtime at UDP session admission.

## Verification and re-evaluation

Current source evidence is the constructor/resolver, handler delegation,
`handle_client_packet`, `check_stream_plugins`, `StreamPluginStore::get`, the two
send-result helpers, and `cleanup_expired_sessions` described above. Existing
inline tests `test_stream_plugin_store_key_resolved_on_unit` and
`test_no_annotation_leaves_store_key_none` cover unit key ownership; shared
resolver tests cover normalization and namespace defaults. Handler test
`delta_schema_sharing_preserves_siblings_and_old_generations` checks old/new
plugin-key generations. UDP timeout/cleanup tests cover the constant and
inactive/active session examples. These test sources do not prove cross-process
policy hot reload or a live-session revocation bound; this documentation
correction does not claim a fresh integration run.

Re-evaluate a wiring report against `UdpRouteUnit::from_route`, the shared
resolver, the selected-unit key passed by `handle_client_packet`, and the current
store lookup in `check_stream_plugins`. Historical resolution is not grounds to
reject a regression with current evidence. Re-evaluate the session contract if
immediate policy revocation or a maximum lifetime becomes a requirement; that
requires an explicit lifecycle design rather than assuming idle expiry supplies
the guarantee.

## Resolution references

- udp-F012 / F-012: originally recorded fixed on 2026-05-26. The old record
  described adding a schema-side key, resolving it in the handler, and retaining
  a vestigial runtime field. Those ownership descriptions are historical, not
  current fields or implementation requirements. The selected historical fix
  implemented UDP support instead of the alternative controller warning/status
  for an unsupported annotation.
- F-016: established-session fast-path correction, recorded on 2026-05-26;
  current owning knowledge is linked above.
- B0214: corrected current route-unit ownership, removed the inaccurate
  60-second revocation claim, and repaired the F-016 knowledge link. Runtime and
  integration-test behavior are unchanged.
- `clarify-stream-plugin-udp-architecture-coverage`: reconciled the architecture,
  annotation, and user documentation with the verified split between UDPRoute
  session admission and the unsupported Gateway-level UDP listener attachment.
  No runtime or integration-test behavior changed.
