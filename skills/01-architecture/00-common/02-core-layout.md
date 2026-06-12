---
name: core-layout
description: Core module layering conventions — top-level grouping, per-bin layout, placement rules, anti-patterns.
---

# Core layering conventions

> This file is the placement reference for adding new code.

## Top-level grouping

`src/core/` has five top-level groups:

```
src/core/
├── controller/   # Logic belonging to edgion-controller
├── gateway/      # Logic belonging to edgion-gateway
├── ctl/          # Logic belonging to edgion-cli
├── center/       # Logic belonging to edgion-center (federation center)
└── common/       # Code shared across bins
```

**Placement principles**:
- Serves the control plane only → `controller/`
- Serves the data plane only → `gateway/`
- Serves only the CLI → `ctl/`
- Serves only the federation center → `center/`
- Depended on by at least two bins with no implicit runtime coupling → `common/`

## Gateway layout

Organized by subsystem, not by technical primitive:

```
src/core/gateway/
├── api/          # Admin / test API
├── backends/     # Backend discovery / health checks / BackendTLSPolicy
│   ├── discovery/    # Endpoint / EndpointSlice / Service discovery
│   ├── health/       # Active health checks
│   ├── policy/       # Backend TLS policy
│   ├── preload/      # LB preloading
│   └── validation/   # Endpoint validation
├── cache/        # LRU cache
├── cli/          # CLI entry + Pingora startup
├── config/       # GatewayClass / EdgionGatewayConfig handlers and storage
├── conf_sync/    # gRPC config client + ClientCache
│   ├── conf_client/  # gRPC client implementation
│   └── cache_client/ # Local cache + event dispatch
├── lb/           # Load-balancing policy implementations
│   ├── backend_selector/  # Weighted round-robin selector
│   ├── ewma/              # EWMA algorithm
│   ├── leastconn/         # Least connections
│   ├── lb_policy/         # LB policy definitions
│   ├── runtime_state/     # Per-backend runtime state
│   └── selection/         # Algorithm factory
├── link_sys/     # External system providers + runtime store
│   ├── providers/    # Redis / Etcd / ES / Webhook / LocalFile
│   └── runtime/      # LinkSysStore + DataSender
├── observe/      # AccessLog / Metrics / SSL/TCP/TLS/UDP logs
├── path_normalization/ # Request path normalization
├── plugins/      # Plugin system
│   ├── http/         # 28 HTTP plugin implementations
│   ├── stream/       # Stream plugins (IP restriction + TLS routing)
│   └── runtime/      # PluginRuntime + conditions + Gateway API filters
├── routes/       # Route processing
│   ├── http/         # HTTPRoute matching + proxy_http
│   ├── grpc/         # GRPCRoute matching + gRPC integration
│   ├── tcp/          # TCPRoute runtime
│   ├── tls/          # TLSRoute runtime
│   └── udp/          # UDPRoute runtime
├── runtime/      # Pingora runtime core
│   ├── server/       # Listener builders + error responses
│   ├── matching/     # Gateway/Route/TLS matching
│   └── store/        # Gateway + Route configuration storage
├── services/     # Gateway-side services (ACME challenge)
└── tls/          # TLS management
    ├── boringssl/    # BoringSSL backend (feature)
    ├── runtime/      # Downstream/upstream TLS callbacks
    ├── store/        # Certificate store + SNI matching
    └── validation/   # Certificate validation
```

**Second-level rules**:
- `runtime/` — Pingora runtime core only
- `routes/` — route managers, matchers, protocol services only
- `plugins/` — plugin implementations and execution framework only
- `backends/` — upstream discovery, health filtering, backend policies only
- `tls/` — downstream TLS certificate handling and validation only
- `link_sys/` — external systems declared by LinkSys only
- `config/` — only configuration resources that affect the gateway runtime; do not place gRPC sync mechanisms here

## Controller layout

The `api/` handlers are flat `.rs` files; `workqueue`, `processor_registry`, and `schema_validator`
are also flat `.rs` files, not directories. The canonical, fully annotated tree lives in
`../01-controller/00-overview.md`; this is the placement-oriented summary.

```
src/core/controller/
├── api/          # Admin API endpoints (flat .rs files)
│   ├── cluster_handlers.rs       # Cluster-scoped resource CRUD
│   ├── namespaced_handlers.rs    # Namespaced resource CRUD
│   ├── configserver_handlers.rs  # ConfigServer endpoints (for ctl)
│   ├── region_route_handlers.rs  # RegionRoute endpoints
│   ├── wipe_handlers.rs          # Wipe-all endpoints
│   ├── debug_handlers.rs / diagnostics_handlers.rs # Debug / diagnostics
│   ├── authz_classifier.rs / authz_middleware.rs / cli_token_middleware.rs # Authz
│   └── common.rs                 # Shared handler logic
├── authz/        # Authorization engine
├── cli/          # CLI entry and startup
├── conf_mgr/     # Configuration manager (core)
│   ├── manager.rs                # ConfMgr facade
│   ├── processor_registry.rs     # Global processor registry
│   ├── schema_validator.rs       # CRD Schema validation
│   ├── conf_center/              # Configuration source abstraction
│   │   ├── file_system/          #   FileSystemCenter implementation
│   │   └── kubernetes/           #   KubernetesCenter implementation
│   └── sync_runtime/             # Sync runtime
│       ├── workqueue.rs          #   Work queue
│       ├── metrics.rs            #   Prometheus metrics
│       ├── shutdown.rs           #   Graceful shutdown
│       └── resource_processor/   #   Resource processing pipeline
│           ├── handlers/         #     20 resource handlers
│           ├── ref_grant/        #     Cross-namespace reference validation
│           ├── secret_utils/     #     Secret dependency tracking
│           └── configmap_utils/  #     ConfigMap utilities
├── conf_sync/    # Config sync (gRPC server side)
│   ├── conf_server/              # ConfigSyncServer + gRPC implementation
│   └── cache_server/             # ServerCache + EventStore
├── fed_sync/     # Federation sync toward Center
├── observe/      # Logging facade
└── services/     # Controller-side services
    └── acme/                     # ACME certificate automation
```

**Placement rules**:
- K8s watch/list, parse/preparse, reference management, status writeback → `conf_mgr/`
- Configuration distribution toward the Gateway → `conf_sync/`

## Center layout

The federation Center binary (edgion-center) and its `src/core/center/` tree have been extracted to the
separate EdgionCenter repository. The Controller side still depends on the shared federation-sync proto in
`common/fed_sync/`; cross-bin types live in `types/`.

## Common layout

Deliberately kept small:

```
edgion-common/src/
├── auth/         # OIDC JWT authentication state (AuthMiddlewareState, JWKS cache)
├── local_auth/   # Local HS256 JWT authentication (login, token issuance)
├── unified_auth/ # Unified authentication middleware (OIDC + local sequential validation)
├── api/          # Admin API route composition utilities (compose, auth-layer wrapping)
├── jwt_common.rs # Common JWT/JWK utility functions (algorithm policy, JWK selection, error mapping)
│                 #   ← both common and gateway depend on this module; gateway must not be reverse-depended on by common
├── conf_sync/    # Shared proto / traits / sync types
│   ├── proto/        # protobuf definitions
│   ├── traits/       # CacheEventDispatch + ConfHandler
│   └── types/        # Sync-related types
├── fed_sync/     # Shared federation-sync proto / types (Controller ↔ Center)
├── observe/      # Shared observability helpers (logging facade)
├── config/       # Shared startup configuration
├── startup.rs    # Common process startup helpers
├── grpc_tls.rs   # Shared gRPC TLS setup
├── http_util.rs  # Shared HTTP utilities
├── matcher/      # Shared matching algorithms
│   ├── host_match/   # Domain matching (Hash + Radix)
│   ├── ip_radix_tree/ # IP Radix tree
│   └── radix_tree/   # Generic Radix tree
└── utils/        # Reusable utilities
    ├── net/              # IP validation, parsing
    ├── duration/         # Duration parsing
    ├── real_ip_extractor/# Real-IP extraction
    ├── metadata_filter/  # Metadata cleansing
    └── proxy_protocol/   # PROXY Protocol
```

**Dependency direction constraints**:
- `common` must not depend on `gateway`
- `common` must not depend on `controller` (2 historical legacy exceptions remain to be cleaned up; see below)

Types shared across bins (such as RegionRoute Entry types) must live in `types/`, so each side depends on `types` rather than depending on each other. JWT/JWK utilities and other logic shared by both sides must live in `common/`, with gateway/center depending on it.

**Known historical legacy exceptions (`common → controller`, to be cleaned up later):**
- `common/config/mod.rs` → `controller::conf_mgr::{ConfCenterConfig, FileSystemConfig}`
- `common/utils/metadata_filter.rs` → `controller::conf_mgr::conf_center::MetadataFilterConfig`

When you encounter these two places do not mistake them for new violations; fixing them requires lifting the related configuration/traits up into `types/` or `common/`, which is a separate task.

Do not move code into `common/` simply to avoid a cross-module import. Keep ownership clear by business domain.

## Types layout

```
edgion-resources/src/
├── resource/         # Resource system infrastructure
│   ├── kind/         #   ResourceKind enum
│   ├── defs/         #   define_resources! macro declarations
│   ├── macros/       #   Macro definitions
│   ├── meta/         #   ResourceMeta trait
│   └── registry/     #   Resource type registry
├── resources/        # Per-kind resource structs (corresponding to CRDs)
│   ├── gateway/      #   Gateway
│   ├── http_route/   #   HTTPRoute
│   ├── edgion_plugins/#  EdgionPlugins
│   └── ...           #   The remaining 17 resources
├── common/           # KeyGet/KeySet unified accessors
├── constants/        # Centralized constants
│   ├── app/          #   Application identifiers
│   ├── labels/       #   K8s labels
│   ├── annotations/  #   K8s annotations
│   ├── headers/      #   HTTP headers
│   └── secret_keys/  #   Secret data keys
├── ctx.rs            # EdgionHttpContext
├── filters.rs        # PluginRunningResult
├── schema.rs         # JSON Schema
├── err.rs            # Error types
├── work_dir.rs       # Working directory
└── observe.rs        # Observability configuration
```

## Anti-patterns

Avoid reintroducing the following patterns:

| Anti-pattern | Correct approach |
|---------|---------|
| Top-level `src/core/api`, `src/core/cli`, `src/core/conf_sync` | Place under the corresponding bin group |
| Flat gateway buckets such as `gateway/http_routes`, `gateway/health_check` | Organize by subsystem |
| New compatibility shims that hide the real ownership | Place directly in the correct bin group |
| Putting execution logic in `types/` | types only holds data definitions |
| `common` or `center` importing internal `controller` modules | Lift shared types into `types/`, have each side depend on types |
| Calling kube-rs API directly inside the gateway | Sync from the Controller via ConfigClient |
| Plugins accessing databases / external systems directly | Go through the client in LinkSysStore |

If a new subsystem needs ownership, prefer creating a clear directory under the correct bin group rather than adding yet another cross-cutting top-level bucket.
