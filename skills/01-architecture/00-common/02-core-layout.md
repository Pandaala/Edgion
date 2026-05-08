---
name: core-layout
description: Core module layering conventions — top-level grouping, per-bin layout, placement rules, anti-patterns.
---

# Core layering conventions

> This file is the placement reference for adding new code.

## Top-level grouping

`src/core/` has only four top-level groups:

```
src/core/
├── controller/   # Logic belonging to edgion-controller
├── gateway/      # Logic belonging to edgion-gateway
├── ctl/          # Logic belonging to edgion-cli
└── common/       # Code shared across bins
```

**Placement principles**:
- Serves the control plane only → `controller/`
- Serves the data plane only → `gateway/`
- Serves only the CLI → `ctl/`
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

```
src/core/controller/
├── api/          # Admin API endpoints
│   ├── cluster_handlers/      # Cluster-scoped resource CRUD
│   ├── namespaced_handlers/   # Namespaced resource CRUD
│   ├── configserver_handlers/ # ConfigServer endpoints (for ctl)
│   └── common/                # Shared handler logic
├── cli/          # CLI entry and startup
├── conf_mgr/     # Configuration manager (core)
│   ├── conf_center/           # Configuration source abstraction
│   │   ├── file_system/       #   FileSystemCenter implementation
│   │   └── kubernetes/        #   KubernetesCenter implementation
│   ├── sync_runtime/          # Sync runtime
│   │   ├── workqueue/         #   Work queue
│   │   ├── resource_processor/#   Resource processing pipeline
│   │   │   ├── handlers/      #     23 resource handlers
│   │   │   ├── ref_grant/     #     Cross-namespace reference validation
│   │   │   ├── secret_utils/  #     Secret dependency tracking
│   │   │   └── configmap_utils/#    ConfigMap utilities
│   │   ├── shutdown/          #   Graceful shutdown
│   │   └── metrics/           #   Prometheus metrics
│   ├── processor_registry/    # Global processor registry
│   └── schema_validator/      # CRD Schema validation
├── conf_sync/    # Config sync (gRPC server side)
│   ├── conf_server/           # ConfigSyncServer + gRPC implementation
│   └── cache_server/          # ServerCache + EventStore
├── observe/      # Logging facade
└── services/     # Controller-side services
    └── acme/                  # ACME certificate automation
```

**Placement rules**:
- K8s watch/list, parse/preparse, reference management, status writeback → `conf_mgr/`
- Configuration distribution toward the Gateway → `conf_sync/`

## Common layout

Deliberately kept small:

```
src/core/common/
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
├── config/       # Shared startup configuration
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
- `common` must not depend on `controller` (3 historical legacy exceptions remain to be cleaned up; see below)
- `center` must not depend on `controller`

Types shared across bins (such as RegionRoute Entry types) must live in `types/`, so each side depends on `types` rather than depending on each other. JWT/JWK utilities and other logic shared by both sides must live in `common/`, with gateway/center depending on it.

**Known historical legacy exceptions (`common → controller`, to be cleaned up later):**
- `common/config/mod.rs` → `controller::conf_mgr::{ConfCenterConfig, FileSystemConfig}`
- `common/utils/metadata_filter.rs` → `controller::conf_mgr::conf_center::MetadataFilterConfig`
- `common/local_auth/secret.rs` → `controller::conf_mgr::conf_center::traits::{CenterApi, ConfWriterError}`

When you encounter these three places do not mistake them for new violations; fixing them requires lifting the related configuration/traits up into `types/` or `common/`, which is a separate task.

Do not move code into `common/` simply to avoid a cross-module import. Keep ownership clear by business domain.

## Types layout

```
src/types/
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
