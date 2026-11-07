---
name: managed-outbound-per-destination-health
description: Use when reviewing ManagedOutboundClient health, passive ejection, active probes, retry endpoint selection, or countTimeout for ForwardAuth, route auth, Guardrail/AiGuard decision services and Webhook LinkSys.
knowledge_type: regression-guards
---

# Managed outbound health is per destination and always time-bounded

## Conclusion and scope

`plugins/http/common/outbound/health.rs::OutboundHealth` owns health for every
`ManagedOutboundClient`. A destination is one ready endpoint of a
`Service`/`ServiceEndpointSlice` target, or the single destination of a URL or
`ServiceClusterIp` target. Each destination owns the canonical
`backends::outlier_detection::PassiveEjectionState` plus an independent active
verdict. The call path (`call.rs`) selects one usable destination per attempt.

Required behavior:

- One failing endpoint is skipped; it never gates the rest of the pool. A call
  fails fast as `Unhealthy` only when no destination is usable.
- Passive ejection is always time-bounded: `ejectionTime × ejection count`,
  capped by `maxEjectionTime`, then one half-open probe lease. There is no
  configuration (for example passive without a backoff block) that latches a
  destination until the resource is rebuilt.
- All passive failure kinds use one `FailureClass` so a single consecutive
  counter (`unhealthyThreshold`, admitted `>= 1`) decides ejection. Separate
  per-class counters would let alternating 503 / timeout / 500 never eject.
- `maxEjectionPercent` (default 50, `0` = no cap, minimum one ejectable) bounds
  ejection against the current ready set, arbitrated under one lock on the
  failure path, mirroring the backend and AI credential wrappers.
- `countTimeout: false` stops tokio-deadline, reqwest and body-read timeouts
  from counting. Connect/transport and other body-stream errors always count. A
  response above `maxResponseBytes` is a local limit and records nothing.
- Every attempt records its outcome on its own destination. A retry re-picks
  excluding tried endpoints (Envoy `previous_hosts`) and retargets a clone of
  the built request with `set_ip_host`/`set_port`; templates are not re-rendered.
  When nothing else is usable (and always for static and authority calls), the
  retry stays on the previous destination only if it is re-admitted; a
  destination the previous attempt ejected (or that went actively unhealthy)
  ends the call with `Unhealthy` instead of being retried.
- Active probes cover every destination each tick (bounded concurrency inside
  the abort-only task). Active and passive are independent signals; active
  success at `healthyThreshold` also lifts a passive ejection.

## Rationale and evidence

The former model kept one `healthy` bit per client: one bad pod could fail every
call to a healthy pool, alternating active probes never removed a bad pod, a
passive-only configuration without `backoff` latched forever, `countTimeout` was
never read, and retries reused the failed endpoint (gw-plug3-01). Envoy outlier
detection, NGINX `max_fails`/`fail_timeout` and Kong/APISIX target health are
per host and recover automatically; this design follows them while reusing the
existing canonical state machine instead of a second circuit implementation.

Request selection is lock-free for closed destinations: an `ArcSwap` snapshot
is rebuilt only when the published `ResolvedServiceSection` `Arc` for the Service
port changes (`EpSliceStore::ready_section_for_port`), carrying over state of
remaining endpoints and pruning departed ones. A `maybe_open` atomic mirror gates
the locked `admit()` call; a success after failures clears it and re-checks
`is_open()` to avoid losing a concurrent reopen.

## Alternatives and limits

- **Upstream-authority mode (deliberate scope).** A Service anchor whose
  EdgionBackendTrafficPolicy sets `upstreamAuthority` dials through
  `AnchorPinnedResolver` and reqwest's per-authority pool (HTTP/2 multiplexes
  all calls on one connection), so the endpoint is not chosen per call. Such
  calls share one pool-level destination with the same bounded ejection and
  half-open probe; active probes use `healthCheckHost`. Resolver filtering was
  rejected because it cannot eject a pod behind an established connection and
  cannot carry a probe lease. Per-endpoint authority clients are tracked in
  `tasks/todo/gw-plug3-01b-upstream-authority-per-endpoint-health.md`.
- No panic routing: with every destination unusable the call fails fast. A panic
  threshold would be a new failure-policy shape (design rule F freeze); for
  fail-closed auth consumers it adds latency without security benefit.
- Ejection duration is linear and the ejection count does not decay; this is
  inherited from the canonical state (bounded by `maxEjectionTime`).
- Active un-eject can let an endpoint with a healthy probe path but failing real
  calls flap between ejected and admitted.
- Health lives in the client; a client rebuild still resets it (gw-plugrt-02).
- Probe load is now destinations × clients per interval.
- Concurrent success/failure on one destination can let one interleaved success
  skip its counter reset (`failures_pending` is set after the increment); it is
  self-healing at the next success and consecutive counting is loose under
  concurrency anyway.
- Authority calls run before the health and rate gates render the authority, so
  `edgion_gateway_upstream_authority_rejected_total` also counts calls that would have
  been `Unhealthy`.

## Verification and re-evaluation

Unit tests: `edgion-gateway/tests/unit/plugins/http/common/outbound/health_test.rs`
(time-bounded passive-only recovery, single probe lease, dropped/ignored probe,
counter reset, percent cap, tried-endpoint exclusion, per-endpoint active
verdicts and un-eject, unsafe endpoints, churn carry-over, authority pool) and
`edgion-resources/tests/unit/resources/link_sys/webhook_test.rs`
(`test_passive_ejection_*`). Re-evaluate when the canonical
`PassiveEjectionState` semantics change, when authority mode gains per-endpoint
clients, or when outbound health moves out of the client (gw-plugrt-02).

## Resolution references

- gw-plug3-01 (2026-11-07, feature-10-28): single latching circuit per pool,
  dead `countTimeout`, retries pinned to the failed endpoint — fixed.
- Supersedes [passive-only recovery timestamp CAS](../../concurrency/superseded/halfopen-probe-toctou-cas.md) (F-B02).
