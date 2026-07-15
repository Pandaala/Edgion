---
name: controller-gateway-link
description: Bidirectional gRPC sync architecture between Controller and Gateway — ConfigSync protocol, Watch/List mechanism, implementation on both sides.
---

# 03 Controller↔Gateway Link

> Controller and Gateway communicate via bidirectional gRPC for configuration sync.
> Gateway connects to Controller as a client, fetching the full state via List and incremental updates via Watch.

## File index

| File | Topic | Recommended reading scenario |
|------|------|-------------|
| [00-overview.md](00-overview.md) | ConfigSync gRPC protocol overview | Understanding the sync protocol design, Proto definitions, version tracking |
| [01-controller-side.md](01-controller-side.md) | Controller-side gRPC server | Debugging ConfigSyncServer, WatchObj, ClientRegistry |
| [02-gateway-side.md](02-gateway-side.md) | Gateway-side gRPC client | Debugging ConfigSyncClient, ClientCache, ConfHandler |
| [webhook.md](webhook.md) | Webhook provider (backendRef target + request-templating redesign) | Webhook outbound request shape, origin/custom allow-list, presence policy, controller-side template compile + secret resolution, consumer `path_override` API, health-reset semantics |

## Architecture overview

```
  Controller                                          Gateway
┌─────────────────────────┐                ┌─────────────────────────┐
│                         │                │                         │
│  ResourceProcessor      │                │  ConfigSyncClient       │
│  └── ServerCache<T>     │   gRPC         │  ├── grpc_client        │
│      └── EventStore     │◄──────────────►│  └── ConfigClient       │
│                         │                │      └── ClientCache    │
│  ConfigSyncServer       │   List/Watch   │         └── EventDispatch
│  ├── WatchObj (per-kind)│──────────────►│             └── ConfHandler
│  └── ClientRegistry    │                │                         │
│                         │   WatchMeta    │                         │
│  server_id (per-start)  │──────────────►│  Detects server_id change
│                         │                │  → triggers full relist │
└─────────────────────────┘                └─────────────────────────┘
```

## Core concepts

- **ConfigSync protocol**: defines 4 RPC methods (GetServerInfo, List, Watch, WatchServerMeta) that Gateway uses to sync resource configuration with Controller.
- **Version tracking**: `sync_version` (monotonically increasing) and `server_id` (a monotonic
  millisecond string per published server instance) identify a per-kind List/Watch generation.
- **Default non-synced resources**: `ReferenceGrant` and `Secret` are the default
  `no_sync_kinds`; an explicit list replaces that default. ConfigMap is currently advertised by
  the Controller unless excluded, but the Gateway intentionally has no ConfigMap cache.
- **Channel security (mTLS)**: `conf_sync_security` is a **required** section — omitting it makes both binaries refuse to start (fail-close, 2026-07-14), because this channel carries resolved Secrets. The current YAML contract uses the `conf_sync_security.certs` list selected by `active`. The old singular `conf_sync_security.tls` field still accepted by code is pre-release removal debt; do not use or extend it. Certificates are generated via `edgion-cli gen-certs --cert-name <name>`. See [00-overview.md § Channel security](00-overview.md) for details.
- **Module distribution**: `edgion-common/src/conf_sync` (Proto, traits, transport config),
  `edgion-controller/src/conf_sync` (server), `edgion-gateway/src/conf_sync` (client), and
  `edgion-cli/src/certs` (certificate generation).

## Testing infrastructure

- **Wipe-all test path**: `POST controller /api/v1/admin/wipe-all` (testing-mode only) deletes
  every current `ResourceKind` in tier order; the coverage test currently asserts all 21 concrete
  kinds. See [05-testing/02-cache-residue-detection.md](../../05-testing/02-cache-residue-detection.md).
