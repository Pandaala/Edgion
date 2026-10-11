---
id: ARCH-118
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-426
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Bind gateway presence to live metadata registrations

## Current ownership and concrete trigger

ClientRegistry represents each client_id with one empty ClientMeta value. register replaces that slot without counting or identifying the registering session; unregister removes it unconditionally. ConfigSyncGrpcServer::watch_server_meta registers once per RPC and independently unregisters on each stream's exit. The registration API therefore cannot express whether another active metadata stream still owns the same gateway's presence.

For example, retain metadata streams for gateways G, H and J. Let G open a second stream with its existing client_id, then terminate G's first stream. The second registration leaves three distinct keys, while the first stream's cleanup removes G's key and broadcasts a count of two. G's surviving stream has no registration left and does not register again. A later disconnection of that second stream also removes nothing. The minimum-one clamp hides an empty registry but does not repair the three-to-two witness.

This overlap is relevant to the real producer: ConfigSyncClient::run_leader_session retains self.client_id across attempts, drops an old stream after installed-generation cancellation and immediately reconnects for intentional reload. Remote RPC cleanup is a separately scheduled task and is not awaited before the next registration. Same-root TLS rotation deliberately preserves established connections. The witness needs a replacement RPC to register before old cleanup; it does not require two startup metadata coordinators or permanent standby connections.

The actual Gateway metadata consumer sets the shared gateway-instance count on every received event. RateLimitLocalConfig::get_effective_rate divides Cluster scope by that count, retaining Instance scope separately. An incorrect registry count can therefore change best-effort per-instance allowances until another registration or controller replacement repairs it. This is not a claim that the product provides exact globally coordinated quotas, or that an incident was reproduced.

## Bounded improvement and migration

Make each metadata RPC acquire a private registration lease from ClientRegistry. Track the number of live leases per client_id and count distinct occupied keys, preserving count().max(1). Release the exact lease when its RPC task exits; remove the key and notify only when its last lease ends. A small private RAII guard can own cleanup across initial-send failure, receiver closure, reload, term/process cancellation and task cancellation instead of repeating key-only unregister calls in every branch. No data-plane allocations or request locks are added; registration is a cold connection boundary.

Preserve the deliberate one-process metadata coordinator and client_id identity across reconnection. Update both ClientRegistry and watch_server_meta together; a registry-only counter cannot help if the RPC still releases by unqualified key. Keep metadata observability, unique-ID counting, same-ID registration idempotence for the count, current all-serve/leader-only scope and best-effort rate-limit policy. Do not replace distinct IDs with total stream count, add heartbeat leases/timeouts, redesign cluster membership or create a second global registry.

A generation-qualified latest-owner slot is a smaller alternative if superseding registration explicitly revokes the predecessor and the contract proves only the newest session matters. Without that contract, latest-session exit could erase an older still-live registration. Counting live leases directly expresses the existing connected-gateway purpose; evaluate that small cold counter cost during implementation review.

## Original evidence and required verification

Primary: edgion-controller/src/conf_sync/conf_server/client_registry.rs:11–54. Actual caller: grpc_server.rs:762–900, including each key-only cleanup. Actual Gateway producer/consumer: edgion-gateway/src/conf_sync/conf_client/grpc_client.rs:538–686; installed generation cancellation and same-ID immediate reconnection. RateLimitLocalConfig's effective_rate_for_instances at edgion-resources/src/resources/edgion_plugins/plugin_configs/rate_limit_local.rs:484–505 establishes the bounded consequence.

Original Controller-side link architecture describes distinct connected Gateway counting, and transactional-controller-channel-selection explicitly avoids extra metadata registration for candidate probes. Neither forbids a transient replacement/cleanup overlap or gives old RPC cleanup authority over another active owner. The accepted Watch send-backpressure decision does not supply registration ownership. Existing client_registry_test.rs proves repeated registration leaves the count unchanged, but never releases one of two same-ID owners. grpc_server_test.rs covers ordinary metadata shutdown/reload/term cancellation and sequential TLS rotation; none establishes replacement cleanup ownership.

Future tests must retain two same-ID sessions plus two other IDs, end each same-ID session in both orders, and observe the count remains three until the last ends. Exercise the actual RPC receiver-closure path, replacement after a TLS channel reload, initial send failure and task drop/abort cleanup. Preserve unique-client counting, minimum-one behavior, notifications only on distinct-count transitions, and existing term/process/reload tests. No tests/builds/network applications were run. This is independent from the separately examined metadata-observation notification gap and ARCH-087's typed resource watcher.
