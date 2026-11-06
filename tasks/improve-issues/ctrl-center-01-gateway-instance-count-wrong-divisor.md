# ctrl-center-01: Cluster-scope rate-limit divisor (`gateway_instance_count`) is per-controller-replica and controller-wide, not per Gateway fleet

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_sync/conf_server/client_registry.rs:81` (`ClientRegistry::count`)
  - `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:769-835` (`watch_server_meta` registers and pushes the count)
  - `edgion-controller/src/conf_sync/conf_server/config_sync_server.rs` (`ConfigSyncServer::new` creates a fresh empty `ClientRegistry` per serving epoch)
  - `edgion-common/src/conf_sync/proto/config_sync.proto` (`WatchServerMetaRequest { client_id, client_name }` only)
  - `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs:620` (`set_gateway_instance_count`)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/rate_limit_local.rs:36-66` (`RateLimitLocalScope::Cluster`)
  - `edgion-controller/src/config/conf_center/kubernetes.rs:14-20` (`HaMode::AllServe`)

## Summary

`RateLimitLocal` with `scope: Cluster` computes the per-instance rate as
`ceil(rate × skewTolerance / gateway_instance_count)`. That count is the number of distinct
`client_id`s registered in the **ConfigSyncServer of the controller replica the gateway happens to
be connected to**. In `all-serve` HA each replica has its own registry, so every gateway sees only
its replica's share of the fleet; and in every mode the count spans every gateway process connected
to the controller, regardless of which Gateway/GatewayClass deployment it serves. The documented
"total quota for all instances combined" is therefore wrong by an arbitrary factor in both directions.

## Current behavior (evidence)

- `ClientRegistry` is a `HashMap<client_id, refcount>`; `count()` returns `clients.len().max(1)`.
  It is owned by one `ConfigSyncServer` instance (one per replica per serving epoch).
- `watch_server_meta` registers the caller and pushes `registry.count()` on every change.
  The gateway writes it into the process-global `GATEWAY_INSTANCE_COUNT` used on the hot path.
- `all-serve` mode (documented in `skills/01-architecture/01-controller/03-config-center/02-kubernetes/01-ha-mode.md`,
  "Gateway -> any replica (edgion-controller Service)") runs one ConfigSyncServer per replica.
  Example: 3 controller replicas, 6 gateway pods spread 2/2/2 → every gateway sees count=2 →
  each enforces `rate/2` → effective cluster quota is **3× the configured rate**. With uneven
  spreading (e.g. 5/1) the quotas are both over- and under-enforced at once.
- The registration carries no Gateway identity (`WatchServerMetaRequest` has only `client_id`,
  `client_name`; every gateway passes the literal `"edgion-gateway"` as name). A controller that
  serves a 2-pod dedicated Gateway and an 8-pod shared Gateway reports 10 to both, so a Cluster
  limit attached to a route of the 2-pod Gateway is enforced at `rate/10 × 2 = 20%` of the configured value.
- After a leader-only failover or any serving-epoch replacement, the new `ConfigSyncServer` starts
  with an empty registry and gateways re-register over the jittered reconnect window; early
  reconnectors observe count=1 and briefly allow the full `rate × skewTolerance` each.

## Industry comparison

- Envoy local rate limit is explicitly per-instance; cluster-wide limits use the global RLS
  (shared counter), never a control-plane-supplied divisor.
- Kong `rate-limiting` `policy: local` is documented as per-node; the `cluster`/`redis` policies use shared storage.
- Where a divisor approach is used (e.g. Istio/Envoy "local RL tuned by replica count" recipes), the
  divisor is the replica count **of the specific proxy Deployment**, taken from the authoritative
  source (Deployment/EndpointSlice of that gateway), not from per-control-plane connection counts.

## Impact

Operators who pick `scope: Cluster` get silently wrong quotas whenever they run `all-serve`
(the HA mode recommended when "every Controller replica must serve conf_sync") or run more than one
Gateway deployment against one controller. Abuse protection and paid-quota enforcement are off by a
multiple with no signal.

## Suggested direction

1. Scope the count to the gateway fleet that shares the quota: have the gateway send its Gateway
   scope (dedicated Gateway key or "shared"/GatewayClass) in `WatchServerMetaRequest`, and count per scope.
2. Make the count replica-independent: in Kubernetes derive it from the gateway pods themselves
   (e.g. ready endpoints of the gateway Deployment/Service, which every replica already watches as
   EndpointSlices) instead of from connections to *this* replica; for non-K8s single-active modes the
   connection registry remains acceptable.
3. Until fixed, reject or warn on `scope: Cluster` when `ha_mode: all-serve`, and document that the
   divisor is controller-wide.

## Notes

Checked `skills/04-review/rate-limit/` (no page covers the divisor source), `conf-sync/regression-guards/transactional-controller-channel-selection.md`
(only notes WatchServerMeta "affects instance counts"), and the HA-mode architecture page. No `tasks/` entry
references `gateway_instance_count`. Not an accepted design.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P1
- `edgion-controller/src/conf_sync/conf_server/client_registry.rs:80-83`: `count()` = `clients.len().max(1)` of this replica's registry.
- `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs:620`: gateway stores that value via `set_gateway_instance_count` as the global divisor.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/rate_limit_local.rs:36-63`: `Cluster` documented as "total quota for all instances combined" with divisor `gateway_instance_count`.
- `edgion-controller/src/config/conf_center/kubernetes.rs:17-26,84`: `AllServe` runs gRPC on every replica, so each replica counts only its own connections.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1 (unchanged)
- `edgion-controller/src/conf_sync/conf_server/client_registry.rs:81-83`: `count()` is still `clients.len().max(1)` of the per-replica registry; file unchanged since `feature-08-30`.
- `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs:620`: still stores that value via `set_gateway_instance_count`; `WatchServerMetaRequest` unchanged.
- `edgion-controller/src/config/conf_center/kubernetes.rs:14-20`: `HaMode::AllServe` still runs one ConfigSyncServer per replica; no `scope: Cluster` + all-serve warning was added.
