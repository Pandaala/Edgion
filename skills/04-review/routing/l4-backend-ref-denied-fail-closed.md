---
name: l4-backend-ref-denied-fail-closed
description: Use when reviewing TCPRoute, UDPRoute, or TLSRoute backend selection findings that a denied cross-namespace backendRef can still be looked up or dialed; covers why the post-select gate is the authorization boundary and why denied weight stays in the selector.
status: fixed
finding_id: l4-backend-ref-denied-fail-closed
closed: 2026-09-06
---

# Denied L4 Backend Refs Fail Closed — FIXED

## Conclusion

TCPRoute, UDPRoute, TLS terminate, and TLS passthrough must inspect the
Controller-stamped `ref_denied` marker immediately after backend selection and
fail closed before Service-key construction, EndpointSlice lookup, policy
lookup, target logging, or dialing. Preload filtering of denied refs is a
startup optimization only and is not an authorization boundary.

Fix suggestions of the form "drop denied refs from the L4 selector", "retry
TLS onto another backend after a denied pick", or "treat preload exclusion as
enforcement" are **not accepted**.

## Core Rationale

**1. Controller already decided; the Gateway must honor the stamp**

ReferenceGrant is `no_sync_kind`. The Controller stamps `ref_denied` on
`TCPBackendRef` / `UDPBackendRef` / `TLSBackendRef` and syncs the route. HTTP
and gRPC already fail that selected slot (`RouteRules::select_backend` /
gRPC integration). L4 compiled the same refs into WRR selectors but dialed
them after only a group/kind check.

**2. Keep declared weight; fail the selected share**

Denied and invalid backendRefs retain their traffic share. Silently removing
them from `build_backend_selector` would redistribute weight onto allowed
siblings and diverge from
[weighted-invalid-backendrefs-retain-traffic-share.md](weighted-invalid-backendrefs-retain-traffic-share.md).
The shared helper `classify_l4_selected_backend` / `l4_post_select` is
the post-select gate. Denied wins over kind. Non-Service kinds stay in
the selector and fail as `UnsupportedBackend`.

**3. TLS retries are for connect failure, not authorization**

A denied pick must `return`, not `continue` the TLS connect-retry loop.
Retrying would satisfy a denied weight with a legal sibling.

**4. Logs stay fixed and low sensitivity**

TCP `status=RefDenied` / `UnsupportedBackend` plus a fixed
`deny_reason`, UDP the same tokens via `UdpStatus`, and TLS
`status` plus `err_log` carry no ReferenceGrant body, namespace,
group, kind, or endpoint list. TLS retries must `return` on both
`Denied` and `UnsupportedKind`.

## Known residuals — deliberate, not defects

- **An already-pinned UDP session does not re-check `ref_denied`.** UDP pins a
  client to a resolved upstream for the session idle window. Grant revocation
  rebuilds the route unit and fails *new* sessions closed; a still-active
  session keeps sending to the cached address until idle expiry. Tearing down
  live sessions on every route generation is a separate hot-reload contract,
  not this dial-gate fix. TCP and TLS connections are per-accept and pick up
  the new generation on the next connect.

## Fix Suggestions Not Accepted

- Filter `ref_denied` or non-Service kinds out of the three L4 selectors —
  drops declared failure share and makes mixed-weight tests pass against
  the bug.
- `continue` the TLS retry loop after a denied or unsupported-kind pick —
  retries onto an allowed backend.
- Treat `backends/preload` exclusion as the authorization gate — preload is
  startup-only and is not on the dial path.
- Log `RefDenied.target_namespace` / `target_name` / grant reason on the L4
  connection log — topology oracle; Controller status already has the detail.
- Copy three ad hoc `ref_denied.is_some()` checks instead of
  `classify_l4_selected_backend` / `l4_post_select` — the L4 family
  already shares the gate in `routes/common.rs`.
- Tear down live UDP sessions when `ref_denied` flips — existing idle-window
  pin; a new generation already fail-closes new sessions.

## Re-evaluation Triggers

Re-open this decision only if:

- L4 routes gain a named-jump publisher that must address a specific
  backendRef, in which case the jump must validate through a compiled view
  that carries `ref_denied` (see
  [denied-named-jump-fails-closed.md](denied-named-jump-fails-closed.md)).
- Gateway API weighted-invalid-backend semantics change so a denied slot must
  be removed from the selector rather than fail its share.
- A new L4 proxy mode is added that builds a Service key from a selected
  backendRef.

## Reference Cases

- `tasks/todo/l4-backend-ref-denied-fail-closed.md` (enforcement fixed
  2026-09-06; static L4 deny-dial e2e added 2026-09-06; HTTP/gRPC parity
  and grant hot-reload coverage remain a follow-up).
- Source: `edgion-gateway/src/routes/common.rs` (`classify_l4_selected_backend`,
  `l4_post_select`), the three L4 `route_unit.rs` selectors, TCP/UDP
  dial paths, and `apply_tls_l4_post_select` used by both TLS modes.
- Unit tests: classify / `l4_post_select` tables, TLS apply-helper
  serialization, `mixed_denied_and_unsupported_keep_declared_weight`
  (TCP/UDP/TLS), `tcp_status_as_str_matches_log_field`,
  `udp_status_as_str_matches_log_field`.
- Static e2e: `TCPRoute/DeniedBackendRef`, `UDPRoute/DeniedBackendRef`,
  `TLSRoute/DeniedBackendRef` (terminate + passthrough), and
  `TLSRoute/UnknownKindBackend` mixed-share sampling.
- Sibling L7 work:
  [denied-named-jump-fails-closed.md](denied-named-jump-fails-closed.md).
