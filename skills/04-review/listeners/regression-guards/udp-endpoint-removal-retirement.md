---
name: udp-endpoint-removal-retirement
description: Use when reviewing UDP sessions that keep forwarding to a removed backend endpoint, the cleanup-loop discovery check, or the UpstreamEndpointRemoved terminal status.
knowledge_type: regression-guards
---

# UDP sessions are retired when their pinned endpoint is removed

## Conclusion and scope

`EdgionUdpProxy` records, per `ClientSession`, the Service key and section
(`PinnedServiceEndpoint`) that supplied its pinned `upstream_addr`. The existing
10-second cleanup pass loads one service-discovery generation and retires every
session for which that generation proves the endpoint gone:

- the Service key is absent, or published as `Unavailable(MissingService)`;
- the section resolves to `MissingRequestedPort`;
- the resolved section no longer publishes the address (`slot_for_addr` misses).

Retirement ignores activity, uses the generation-aware `retire_session`
(`expiry_check = None`), and writes the terminal session log with
`status: UpstreamEndpointRemoved`. The next client datagram misses the fast path
and creates a new session through the full route, policy and backend pipeline.
Indeterminate states (`Unavailable(InvalidEpoch)` and other invalid sections)
keep the session. Readiness is not consulted: an unready but published endpoint
is not removed.

Route, ReferenceGrant and stream-plugin changes still do not affect established
sessions; see
[L4 established flows are not revoked by configuration changes](../../routing/intentional-design/l4-established-flows-not-revoked-on-config-change.md).

## Rationale and evidence

gw-core-05 found that an active UDP client pinned to a deleted pod was
blackholed indefinitely: expiry was idle-only and every send refreshed activity.
Unlike TCP/TLS, a UDP relay receives no reset when the backend disappears.
Envoy's UDP proxy removes sessions for removed hosts for the same reason.

The check reads the published geometry (`ServiceSelectorGeneration`), the same
source `select_service_endpoint` uses for new sessions, not the runtime health
cell, so a transient runtime/membership mismatch does not retire sessions. The
per-packet fast path is unchanged; the added work is one generation load and one
map lookup plus binary search per session every 10 s, outside the datagram path.
The pinned key costs one string allocation per new session.

## Alternatives and limits

Detection latency is up to one cleanup interval (10 s). Rejected: an
endpoint-change subscription with an `upstream_addr → sessions` index; it would
add a discovery-to-listener callback contract for a 10 s latency gain.
A gateway-side discovery generation that transiently omits a Service (for
example an incomplete controller resync) would retire its sessions; the clients
then recreate sessions, and new-session selection would fail in the same window
anyway. Full sets replace Services and EndpointSlices without a clear-then-fill
step, and partial events are batched by `BATCH_DEBOUNCE` (100 ms), so a slice
delete-and-create spread over more than that window can also retire sessions.

ExternalName Services reach UDP through DNS slices: Gateway-local ones by default,
Controller-published ones in `controller` mode (`system` mode pins a direct backend
per session instead). A DNS
answer set that rotates subsets drops still-valid addresses from the section,
and their sessions are retired; the client loses source-port continuity toward
the backend. This is accepted as equivalent to endpoint removal.

## Verification and re-evaluation

Unit tests in `edgion-gateway/tests/unit/routes/udp/edgion_udp_test.rs`:
`pinned_endpoint_removal_requires_published_proof` (live, rotated address,
removed port, Service deleted with slices remaining, both absent),
`cleanup_retires_active_session_whose_endpoint_was_removed`,
`endpoint_removal_candidate_does_not_retire_replacement_generation`, and
`test_cleanup_keeps_active_sessions` with a live endpoint. `invalid_section_is_indeterminate_and_keeps_session` covers one non-removal (`AmbiguousEndpointPort`)
invalid section; `test_cleanup_expired_sessions` keeps the endpoint published so
the idle path is exercised through `cleanup_sessions`. These use an isolated
`ServiceDiscoveryCoordinator`; no live UDP wire or Kubernetes endpoint-churn
scenario was run. `Unavailable(InvalidEpoch)` and the terminal log `status`
value are verified by source inspection only.

Re-evaluate if the session map key or ownership changes, discovery publication
can transiently drop Services during resync, the cleanup interval changes,
UDP sessions start pinning non-Service backends, or ExternalName DNS rotation
causes observable session churn for UDP clients.

## Resolution references

- gw-core-05 (improve review, P2): endpoint-removal blackholing fixed 2026-11-06.
