---
module: config-sync
title: No ACK/NACK feedback loop — Controller cannot distinguish applied vs. rejected config
severity: high
envoy_reference: xDS ACK/NACK with version_info and error_detail (xDS API v3 DiscoveryResponse / DeltaDiscoveryResponse)
---

# No ACK/NACK feedback loop

## Envoy's design (the proven baseline)

In Envoy xDS every response sent by the management plane requires an explicit
acknowledgment from the data-plane client.  After a Gateway processes a
DiscoveryResponse it immediately sends back a DiscoveryRequest that carries:

- `version_info` — the version the client successfully applied (on success), or
  the *last known-good* version (on rejection).
- `error_detail` — a `google.rpc.Status` message populated only on rejection,
  describing exactly which resource was invalid and why.

This gives the control plane observable visibility into what state each data-plane
node is actually running.  If a node NACKs (keeps advertising the old version),
the operator immediately knows that a new resource is bad *and which node rejected
it*, without having to guess from logs.  It also means the control plane can track
per-node applied versions and avoid re-sending resources a client already has.

## Edgion's current design

Edgion's `WatchResponse` carries only server-to-client fields: `data`, `sync_version`,
`err`, and `server_id`.  The `WatchRequest` that opens the stream carries a single
`from_version` (the version to resume from) but there is no message the Gateway
ever sends *back* to the Controller to report what it applied or whether it
rejected something.

Proto: `edgion-common/src/conf_sync/proto/config_sync.proto` lines 36-51.

The Gateway's watch loop (`start_watch` in
`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs`, lines 270-388)
silently skips any event it cannot parse (`continue;` on parse error at line 308)
or logs an error and continues.  The only feedback path is re-starting the stream
after a disconnection.  There is no structured message telling the Controller "I
received sync_version 42 but rejected resource X because of Y."

## The problem / gap

The Controller has no way to know whether a Gateway has actually applied a new
resource version.  Specifically:

1. **Silent config failures are invisible to operators**: if a resource is
   rejected on the Gateway side (parse error, semantic error, dependency missing)
   the Controller sees a healthy gRPC stream and never knows.  The resource
   appears synced in Controller state, but the Gateway is still running stale
   config.

2. **No last-good-version pinning**: when a bad update arrives Envoy's Gateway
   would NACK and remain on the last-good version until the management plane
   fixes the resource.  Edgion's Gateway on a parse error just skips the event
   and continues; there is no rollback guarantee and no signal to the Controller
   to stop propagating the broken version.

3. **Controller health checks are coarse-grained**: the `/ready` endpoint on the
   Controller reports "all caches are ready" but that describes Controller-side
   state, not whether any Gateway has accepted it.

## Impact

- An operator pushes a misconfigured HTTPRoute.  The Controller accepts it (it is
  syntactically valid K8s), serializes it, and pushes it over the Watch stream.
  The Gateway fails to deserialize the plugin section (`record_config_event_parse_error`
  in `event_dispatch.rs:306`), logs a WARN, and skips the event.  Traffic
  continues matching the old rule.  There is no alert, no status condition updated
  on the HTTPRoute, and no operator-visible signal that anything is wrong.
- Debugging requires correlating gateway logs with controller resource versions
  manually, which is operationally expensive in a multi-gateway fleet.

## Suggested direction

Extend the Watch RPC with a client-side feedback message (or add a separate RPC
`Ack`) that lets the Gateway report: the `sync_version` it last successfully
applied per kind, and an optional `error_detail` string when a resource is
rejected.  The Controller can then expose this per-kind applied version per
Gateway via the admin API and/or reflect it back as a `.status` condition on the
affected K8s resource (analogous to the Accepted/ResolvedRefs conditions Gateway
API already mandates for routes).  On the Gateway side, failed resources should
be retained in their last-good state rather than silently skipped.
