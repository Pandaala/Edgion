---
name: cluster-scope-instance-count-divisor
description: Use when reviewing RateLimitLocal `scope: Cluster` accuracy or the `gateway_instance_count` divisor pushed by the Controller over WatchServerMeta (ClientRegistry, all-serve HA, multiple Gateway fleets).
knowledge_type: intentional-design
---

# RateLimitLocal Cluster-scope divisor is a per-controller connection count — ACCEPTED TRADEOFF

## Conclusion and scope

`RateLimitLocal` with `scope: Cluster` enforces
`ceil(rate × skewTolerance / gateway_instance_count)` per Gateway process. The divisor is
the number of distinct `client_id`s registered in the `ClientRegistry` of the
`ConfigSyncServer` the Gateway is currently connected to
(`edgion-controller/src/conf_sync/conf_server/client_registry.rs`, `count()`), pushed
over `WatchServerMeta` and stored in the process-global `GATEWAY_INSTANCE_COUNT`
(`edgion-resources/src/rate_limit_local_config.rs`). Before the first push, and while no
controller pushes, the divisor is the static gateway config value
`rate_limit_local.gateway_instance_count` (default 1).

The user accepted this as an **order-of-magnitude** quota, not a precise cluster-wide
counter (2026-11-06, finding ctrl-center-01). Deviations of a small multiple of the
configured rate are within the intended contract; precise cluster-wide quotas belong to
`RateLimitRedis`, which shares one counter.

Known deviation sources, all accepted:

- **`ha_mode: all-serve`**: every replica runs its own `ConfigSyncServer` and registry,
  so each Gateway sees only the Gateways connected to its replica. Over-admission is
  bounded by roughly the controller replica count (for example 3 replicas, 6 Gateways
  spread 2/2/2 → each enforces `rate/2` → about 3× in total). Uneven spreading mixes
  over- and under-admission.
- **Serving-epoch replacement / leader failover**: the new registry starts empty.
  On an in-process epoch replacement, every open `WatchServerMeta` stream receives a
  terminal switch event carrying `new_server.client_registry().count()` (usually 1),
  and the Gateway applies it before checking `server_id`
  (`edgion-controller/src/conf_sync/conf_server/grpc_server.rs`, server-id check;
  `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs`), so **all** connected
  Gateways briefly use count=1. On a leader-only failover the old stream just drops,
  the last count is kept, and only early reconnectors observe count=1. Either way the
  transient over-admission can reach about fleet size × rate × skewTolerance, and it
  lasts until the Gateways reconnect (immediately after an in-process switch; after
  the jittered backoff on failover; seconds either way). Accepted under the same
  order-of-magnitude decision.
- **Multiple Gateway fleets on one controller**: `WatchServerMetaRequest` carries no
  Gateway identity, so dedicated and shared Gateway deployments are counted together.
  A small fleet then enforces less than its configured quota (under-admission).

## Rationale and evidence

- The decisive rationale is the user's acceptance of an order-of-magnitude contract.
  Supporting context: the plugin is labeled approximate
  (`docs/en/user-guide/http-route/filters/edgion-plugins/README.md`), and
  `docs/en/user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md` contrasts
  "Approximate (CMS error)" with "Exact" (that label concerns counting error, not the
  divisor, so it is weak support).
- The data-plane cost stays a relaxed atomic load (one or two per request); no per-fleet bookkeeping,
  proto identity field, or Kubernetes endpoint-derived count is needed.
- Industry local limiters (Envoy local rate limit, Kong `policy: local`) are likewise
  per-instance approximations; exact cluster quotas use shared storage.

## Alternatives and limits

Rejected for now (cross-module cost out of proportion to an approximate limiter):

- Sending a Gateway scope (dedicated Gateway key / shared) in `WatchServerMetaRequest`
  and counting per scope.
- Deriving the divisor from the gateway Deployment/EndpointSlice instead of
  per-replica connections, so it is replica-independent.
- Rejecting or warning on `scope: Cluster` under `all-serve`.

Known documentation gaps, left unchanged by user decision (2026-11-06):

- `docs/en/user-guide/edgion-plugins/rate-limit-local.md` FAQ "Will Cluster mode
  precisely limit to the configured rate?" states throughput stays within
  `rate × skewTolerance` and that `skewTolerance: 1.0` guarantees no exceeding; that
  bound ignores the deviation sources above.
- The same page's FAQ "Should I use Instance or Cluster mode?" recommends Cluster for
  a global quota "(e.g., SLA commitment of 1000 req/s)", which overstates precision
  under the accepted contract. The user accepted leaving this wording; it does not by
  itself reopen the finding.
- `skills/01-architecture/03-controller-gateway-link/01-controller-side.md` and
  `02-gateway-side.md` describe the count as all Gateways connected to "the
  Controller", not per replica or per fleet.

## Verification and re-evaluation

Evidence is code reading on `feature-10-28`; no new tests were run for this decision.

Re-evaluate if any of these occur:

- An operator incident or product requirement shows `scope: Cluster` is actually
  relied on for exact / paid / SLA quotas (the existing doc wording alone, recorded
  above, is not such evidence).
- Multiple Gateway fleets per controller become a supported common deployment and
  under-admission of small fleets is observed.
- The `ClientRegistry` becomes shared across replicas, or `WatchServerMetaRequest`
  gains a Gateway identity.
- `all-serve` deployments run more than 3 controller replicas, or the epoch-switch
  transient (count=1 on all Gateways) is observed to last longer than the reconnect
  window.

## Resolution references

- ctrl-center-01 (`tasks/improve-issues/`, P1 design-error, CONFIRMED and re-verified
  STILL-VALID on `feature-10-28`) — closed as accepted tradeoff on 2026-11-06.
