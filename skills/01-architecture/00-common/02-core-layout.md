---
name: core-layout
description: Core module layering conventions — top-level grouping, per-bin layout, placement rules, anti-patterns.
---

# Core layering conventions

> This file is the placement reference for adding new code.

## Top-level grouping

The code is a Cargo workspace (resolver 2); each plane is its own crate (the old
single-crate `src/core/` layout is gone):

```
edgion-wasm/         # Proxy-Wasm compatibility/profile leaf (external deps only)
edgion-dsl/          # EdgionDSL language core (leaf, no workspace-internal deps)
edgion-resources/    # Shared resource/config schema (depends on edgion-dsl)
edgion-common/       # Code shared across bins (depends on edgion-resources)
edgion-gateway/      # Data plane + `edgion-gateway` binary
edgion-controller/   # Control plane + `edgion-controller` binary
edgion-cli/          # CLI lib + `edgion-cli` binary
edgion-tests/        # Integration harness and test/helper binaries; not a default member
```

The root `Cargo.toml` is a virtual manifest. There is no root `edgion` package;
test binaries such as `test_server`, `test_client`, and `cache_residue` belong to
`edgion-tests` and must be addressed with `-p edgion-tests`.

**Placement principles**:
- Runtime-independent Proxy-Wasm binary analysis or host ABI contracts → `edgion-wasm/`
- Serves the control plane only → `edgion-controller/`
- Serves the data plane only → `edgion-gateway/`
- Serves only the CLI → `edgion-cli/`
- Depended on by at least two bins with no implicit runtime coupling → `edgion-common/`
- Pure resource/config schema (no runtime) → `edgion-resources/`

The federation Center is a separate repository (EdgionCenter). The in-repo
Controller-side federation client and protocol live in
`edgion-controller/src/fed_sync/`; they are not shared Common modules.

## Gateway layout

Organized by subsystem, not by technical primitive:

```
edgion-gateway/src/
├── api/          # Admin / test API
├── backends/     # Backend discovery / health checks / BackendTLSPolicy
│   ├── discovery/    # Endpoint / EndpointSlice / Service discovery
│   ├── health/       # Active health checks
│   ├── policy/       # Backend TLS policy
│   └── circuit_breaker/ # Backend circuit-breaker state
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
│   ├── runtime_state/     # Per-backend runtime state
│   └── selection/         # Algorithm factory
├── link_sys/     # External system providers + runtime store
│   ├── providers/    # Elasticsearch / Kafka / LocalFile / OTLP / Redis / Webhook
│   └── runtime/      # LinkSysStore + DataSender
├── observe/      # AccessLog / Metrics / SSL/TCP/TLS/UDP logs
├── plugins/      # Plugin system
│   ├── http/         # HTTP implementations for EdgionPlugin variants
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
├── tls/          # TLS management
│   ├── boringssl/    # BoringSSL backend (feature)
│   ├── runtime/      # Downstream/upstream TLS callbacks
│   ├── store/        # Certificate store + SNI matching
│   └── validation/   # Certificate validation
└── util/         # Gateway-owned network / proxy-protocol / real-IP helpers
```

Path normalization schema and algorithms live in
`edgion-resources/src/path_normalization/`. The Etcd LinkSys provider lives in
`edgion-common/src/link_sys_core/providers/etcd/`; it is shared core rather than
a Gateway-local provider.

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
edgion-controller/src/
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
│   │   ├── kubernetes/           #   KubernetesCenter implementation
│   │   └── etcd/                 #   EtcdCenter implementation
│   └── sync_runtime/             # Sync runtime
│       ├── workqueue.rs          #   Work queue
│       ├── metrics.rs            #   Prometheus metrics
│       ├── shutdown.rs           #   Graceful shutdown
│       └── resource_processor/   #   Resource processing pipeline
│           ├── handlers/         #     Registry-backed resource handlers
│           ├── ref_grant/        #     Cross-namespace reference validation
│           ├── secret_utils/     #     Secret dependency tracking
│           └── configmap_utils/  #     ConfigMap utilities
├── conf_sync/    # Config sync (gRPC server side)
│   ├── conf_server/              # ConfigSyncServer + gRPC implementation
│   └── cache_server/             # ServerCache + EventStore
├── fed_sync/     # Controller-side federation client, supervisor, and protocol
├── observe/      # Logging facade
└── services/     # Controller-side services
    └── acme/                     # ACME certificate automation
```

**Placement rules**:
- K8s watch/list, parse/preparse, reference management, status writeback → `conf_mgr/`
- Configuration distribution toward the Gateway → `conf_sync/`

## Center layout

The federation Center binary (edgion-center) and its tree have been extracted to the
separate EdgionCenter repository. The Controller-side protocol and client remain in
`edgion-controller/src/fed_sync/`; cross-crate resource schema types live in
`edgion-resources`.

## Common layout

Deliberately kept small:

```
edgion-common/src/
├── cert_util.rs  # Certificate helpers
├── conf_sync/    # Shared proto / traits / sync types + gRPC sync security config
│   ├── config.rs     # ConfSyncConfig / ConfSyncSecurityConfig / ConfSyncTlsConfig
│   ├── proto/        # protobuf definitions
│   ├── traits/       # CacheEventDispatch + ConfHandler
│   └── types/        # Sync-related types
├── dns/          # DNS resolver implementations
├── ext_proc/     # Shared external-processing protocol
├── link_sys_core/# Backend-agnostic LinkSys lifecycle, TLS, health, and Etcd
├── observe/      # Shared observability helpers (logging facade)
├── secret/       # Secret store
├── startup.rs    # Common process startup helpers
├── grpc_tls.rs   # Shared gRPC TLS setup
├── test_mode.rs  # Shared test-mode flags
└── tls_validation.rs # Backend-agnostic TLS validation
```

**Dependency direction constraints** (now enforced by the crate boundary — a
violation is a compile error, not just a convention):
- `edgion-common` must not depend on `edgion-gateway` or `edgion-controller`.
- There is **no** control→data edge: `edgion-controller` does not depend on
  `edgion-gateway`. Plugin validation on resource ingest runs the schema-layer
  `EdgionPlugins::validate_plugins` (in `edgion-resources`), not the gateway's
  `plugins::http::compile_plugins`. The two share the same underlying checks; the
  schema layer now runs the full DSL static pipeline (parse, compile, validate)
  via the shared `edgion-dsl` crate and omits only VM init, which the data plane
  still enforces fail-closed at runtime.
- The config-center schema (`ConfCenterConfig`, `FileSystemConfig`,
  `KubernetesConfig`, `MetadataFilterConfig`, `EtcdConfig`) lives in
  `edgion-controller/src/config/conf_center/` (controller-only — the gateway never
  reads it); the controller's `conf_mgr/conf_center/*/config.rs` modules re-export
  it from there for their runtime use.

Types shared across bins must live in `edgion-resources` (pure schema) or
`edgion-common` (shared runtime infra), so each side depends on those rather than
on each other.

Do not move code into `edgion-common` simply to avoid a cross-crate import. Keep
ownership clear by business domain.

## Types layout

```
edgion-resources/src/
├── resource/         # Resource system infrastructure
│   ├── kind.rs       #   ResourceKind enum
│   ├── defs.rs       #   define_resources! declarations (source of truth)
│   ├── macros.rs     #   Macro definitions
│   ├── meta/         #   ResourceMeta trait
│   └── registry.rs   #   Resource type registry
├── resources/        # Per-kind resource structs (corresponding to CRDs)
│   ├── gateway.rs    #   Gateway
│   ├── http_route.rs #   HTTPRoute
│   ├── edgion_plugins/#  EdgionPlugins
│   └── ...           #   Other registry-backed and helper resource modules
├── common/           # KeyGet/KeySet unified accessors
├── constants/        # Application, label, annotation, and Secret-key constants
├── credentials/      # Secret-material types and resolvers
├── matcher/          # Shared host/IP/radix matching algorithms
├── path_normalization/ # Shared path-normalization schema and implementation
├── filters.rs        # PluginRunningResult
├── schema.rs         # JSON Schema
├── err.rs            # Error types
├── work_dir.rs       # Working directory
└── observe.rs        # Observability configuration
```

`EdgionHttpContext` is request-runtime state and therefore belongs to
`edgion-gateway/src/ctx.rs`, not `edgion-resources`.

## Anti-patterns

Avoid reintroducing the following patterns:

| Anti-pattern | Correct approach |
|---------|---------|
| Top-level `src/core/api`, `src/core/cli`, `src/core/conf_sync` | Place under the owning crate |
| Flat gateway buckets such as `gateway/http_routes`, `gateway/health_check` | Organize by subsystem |
| New compatibility shims that hide the real ownership | Place directly in the correct bin group |
| Putting execution logic in `edgion-resources/` | resources only holds schema/data definitions and schema-layer validation |
| Shared code importing internal `controller` modules | Lift shared types into `edgion-resources` or backend-agnostic helpers into `edgion-common` |
| Calling kube-rs API directly inside the gateway | Sync from the Controller via ConfigClient |
| Plugins accessing databases / external systems directly | Go through the client in LinkSysStore |

If a new subsystem needs ownership, prefer creating a clear directory under the correct bin group rather than adding yet another cross-cutting top-level bucket.
