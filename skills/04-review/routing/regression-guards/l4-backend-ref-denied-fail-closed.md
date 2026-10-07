---
name: l4-backend-ref-denied-fail-closed
description: Use when reviewing TCPRoute, UDPRoute, or TLSRoute backend selection findings that a denied cross-namespace backendRef can still be looked up or dialed; covers why the post-select gate is the authorization boundary and why denied weight stays in the selector.
status: fixed
finding_id: l4-backend-ref-denied-fail-closed
closed: 2026-09-06
knowledge_type: regression-guards
---

# Denied L4 Backend Refs Fail Closed — FIXED

## Conclusion

TCPRoute, UDPRoute, TLS terminate, and TLS passthrough compile their actual owning Route into a private generation and inspect its
Controller-stamped denial immediately after index selection. The selected loan
fails closed before EndpointSlice lookup, backend policy lookup, target logging,
or dialing; canonical Service keys are prepared on the cold path. Preload filtering of denied refs is a
startup optimization only and is not an authorization boundary.

Fix suggestions of the form "drop denied refs from the L4 selector", "retry
TLS onto another backend after a denied pick", or "treat preload exclusion as
enforcement" violate the weighted selected-failure contract.

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
the compiled post-select mapping; `SelectedL4Backend::with_service` gates the
actual resolver closure. Denied wins over malformed targets. Invalid names,
namespaces, groups, kinds and declared ports stay in the selector and fail as
`UnsupportedBackend`.

**3. TLS retries are for connect failure, not authorization**

A denied or malformed pick must `return`, not `continue` the TLS connect-retry loop.
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
- **HTTP/gRPC data-plane parity and grant hot-reload e2e are a follow-up,
  not a reopen of this finding.** Static L4 deny-dial coverage is in place.
  Remaining suites live in
  `tasks/todo/denied-backend-ref-parity-and-grant-hot-reload.md`.

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

Re-evaluate against current source and new evidence, including:

- L4 routes gain a named-jump publisher that must address a specific
  backendRef, in which case the jump must validate through a compiled view
  that carries `ref_denied` (see
  [denied-named-jump-fails-closed.md](denied-named-jump-fails-closed.md)).
- Gateway API weighted-invalid-backend semantics change so a denied slot must
  be removed from the selector rather than fail its share.
- A new L4 proxy mode is added that consumes a selected backend identity or changes the cold-owner boundary.

## Reference Cases

- The DeniedBackendRef status precondition must use the actual fixture topology:
  local Routes are in `edgion-test` and point to `backend`; Kubernetes Routes
  are in `edgion-default` and point to `edgion-backend`. The shared test helper
  selects the Route namespace using `EDGION_TEST_K8S_MODE`, and its fixture-tree
  regression verifies all four TCP/UDP/TLS Routes retain cross-namespace refs.
  A 404 is a failed test precondition, never proof that backend access was denied.
  The 2026-09-10 full run exposed a hardcoded Kubernetes namespace in local
  tests; this is a test identity defect, not evidence that the runtime dial gate failed.
  After correction, all three DeniedBackendRef suites (including both TLS modes)
  passed in the canonical default sweep recorded at
  `tmp/integration_testing/testing_20260910_154454/report.log` (237 passing report
  entries, zero failures/timeouts). The fixture-tree regression passed in the
  91-test library run. Kubernetes namespace contracts were checked against the
  manifests; live Kubernetes was not run.
- Original security todo closed 2026-09-06 after enforcement plus static
  L4 deny-dial e2e. Remaining coverage lives in
  `tasks/todo/denied-backend-ref-parity-and-grant-hot-reload.md`.
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

## Resolution references

- `l4-backend-ref-denied-fail-closed` — 2026-09-06.
