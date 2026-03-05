# 项目总览

> Edgion API Gateway 的整体架构、Crate 结构、代码组织、核心上下文、CLI 工具和关键依赖。

## High-Level Architecture

```
                    ┌──────────────────────────────────────────────────────────┐
                    │                  edgion-controller                       │
                    │                                                          │
  YAML/K8s CRD ──► │  ConfCenter ──► Workqueue ──► ResourceProcessor          │
                    │  (File/K8s)     (per-kind)    (validate/preparse/parse)  │
                    │                                                          │
  edgion-ctl ────► │  Admin API (:5800)   ConfigSyncServer (gRPC :5810)       │
                    └─────────────────────────────┬────────────────────────────┘
                                                  │ gRPC Watch/List
                                                  ▼
                    ┌──────────────────────────────────────────────────────────┐
                    │                  edgion-gateway                          │
                    │                                                          │
                    │  ConfigSyncClient ──► ClientCache ──► Preparse           │
                    │                       (per-kind)                         │
                    │  Pingora Server                                          │
                    │  ├─ ConnectionFilter (TCP-level, StreamPlugins)          │
                    │  ├─ ProxyHttp (HTTP/gRPC lifecycle)                      │
                    │  │  ├─ request_filter     → route match + plugins        │
                    │  │  ├─ upstream_peer      → backend selection + LB       │
                    │  │  ├─ upstream_response  → response plugins             │
                    │  │  └─ logging            → AccessLog                    │
                    │  └─ TCP/UDP/TLS Routes                                   │
                    │                                                          │
                    │  Admin API (:5900)   Metrics API (:5901)                 │
                    └──────────────────────────────────────────────────────────┘
```

## Crate Structure

Single crate (not a workspace), three `[[bin]]` targets:

| Binary | Path | Runtime | Role |
|--------|------|---------|------|
| `edgion-gateway` | `src/bin/edgion_gateway.rs` | Sync (Tokio created internally, Pingora main loop) | Data plane |
| `edgion-controller` | `src/bin/edgion_controller.rs` | `#[tokio::main(multi_thread)]` | Control plane |
| `edgion-ctl` | `src/bin/edgion_ctl.rs` | `#[tokio::main]` | CLI tool |

Example binaries for testing: `test_server`, `test_client`, `test_client_direct`, `resource_diff`, `config_load_validator`.

Default features: `allocator-jemalloc` + `boringssl`.

## Code Architecture: bin / core / types

```
src/
├── bin/                         # Binary entry points (thin wrappers)
│   ├── edgion_gateway.rs        #   → EdgionGatewayCli::run()
│   ├── edgion_controller.rs     #   → EdgionControllerCli::run()
│   └── edgion_ctl.rs            #   → Cli::run()
├── lib.rs                       # Crate root: pub mod core, pub mod types
├── core/                        # All business logic
│   ├── cli/                     # CLI parsing + startup wiring
│   ├── conf_mgr/                # Config management (controller core)
│   ├── conf_sync/               # gRPC sync (server + client + cache)
│   ├── api/                     # HTTP APIs (controller admin, gateway admin, metrics)
│   ├── gateway/                 # Gateway config, listeners, route dispatch
│   ├── routes/                  # HTTP, gRPC, TCP, TLS, UDP route processing
│   ├── plugins/                 # Plugin system (edgion_plugins, stream_plugins, gapi_filters)
│   ├── backends/                # Backend discovery (Service, EndpointSlice, Endpoint)
│   ├── lb/                      # Load balancing (EWMA, LeastConn, WeightedSelector)
│   ├── tls/                     # TLS termination, cert management
│   ├── observe/                 # Logging: access_log, ssl_log, tcp_log, udp_log, metrics
│   ├── link_sys/                # External system connectors (file, ES, Kafka, Redis)
│   ├── matcher/                 # Host matching, IP radix tree
│   ├── services/                # ACME certificate automation
│   └── utils/                   # Duration parsing, metadata filter, networking, real IP
└── types/                       # Shared type definitions
    ├── resource/                # Resource system (define_resources!, ResourceKind, ResourceMeta)
    ├── resources/               # Per-kind resource structs (Gateway, HTTPRoute, EdgionPlugins, ...)
    ├── common/                  # KeyGet/KeySet unified accessors
    ├── constants/               # Annotations, labels, headers, secret keys
    ├── ctx.rs                   # EdgionHttpContext (per-request state)
    ├── filters.rs               # PluginRunningResult, PluginRunningStage, PluginTags
    ├── schema.rs                # JSON schema validation
    └── err.rs                   # Error types
```

**Design principle:** `types/` is pure data definitions (no business logic), `core/` is all logic. Binaries in `bin/` are thin wrappers that parse CLI and call into `core/cli/`.

## EdgionHttpContext — Per-Request State

`src/types/ctx.rs` — the "carry bag" through the entire HTTP request lifecycle:

| Field | Purpose |
|-------|---------|
| `start_time` | Request timing |
| `gateway_info` | Gateway metadata |
| `request_info` | Client addr, remote addr, hostname, path, trace ID, SNI, gRPC metadata |
| `edgion_status` | Error codes accumulated during processing |
| `route_unit` / `grpc_route_unit` | Matched route rule (contains `PluginRuntime`) |
| `selected_backend` / `selected_grpc_backend` | Chosen backend ref |
| `backend_context` | Service name, upstream attempts, connect time |
| `stage_logs` | `Vec<StageLogs>` — plugin logs per execution stage |
| `pending_edgion_plugins_logs` | For nested ExtensionRef plugin execution |
| `plugin_ref_stack` | Cycle detection for nested plugin refs |
| `plugin_running_result` | Current plugin chain result |
| `ctx_map` | `HashMap<String, String>` — plugin-set variables |
| `path_params` | Lazy-extracted route path parameters |
| `hash_key` | Consistent hashing key |
| `try_cnt` | Upstream connection attempt counter |

Created in `new_ctx()`, consumed in `logging()`. Plugins interact via `PluginSession` adapter.

## edgion-ctl CLI

```
edgion-ctl [--server URL] [--socket PATH] [--target center|server|client] <COMMAND>
```

| Command | Target | Description |
|---------|--------|-------------|
| `apply -f <file/dir>` | center | Apply YAML resources (create or update) |
| `get <kind> [name] -n <ns>` | all | Get resources (table/json/yaml/wide output) |
| `delete <kind> <name> -n <ns>` | center | Delete a resource |
| `delete -f <file>` | center | Delete resources from file |
| `reload` | center | Reload all resources from storage |

**Target types:**
- `center` (default) — ConfCenter API on controller (:5800), supports CRUD
- `server` — ConfigServer cache on controller (:5800), read-only
- `client` — ConfigClient cache on gateway (:5900), read-only

Useful for debugging: compare `server` vs `client` to check sync status.

## Testing Infrastructure

| Component | Path | Purpose |
|-----------|------|---------|
| `test_server` | `examples/code/server/test_server.rs` | Multi-protocol echo backend (HTTP, gRPC, WebSocket, TCP, UDP, auth) |
| `test_client` | `examples/code/client/test_client.rs` | Suite-based test runner with `TestSuite` trait |
| `resource_diff` | `examples/code/validator/resource_diff.rs` | Controller ↔ Gateway sync verification |
| `run_integration.sh` | `examples/test/scripts/integration/` | Full integration test orchestrator |
| Test configs | `examples/test/conf/` | YAML resources organized by `Resource/Item/` |
| Port registry | `examples/test/conf/ports.json` | Unique port allocation per test suite |

See [03-testing/00-integration-testing.md](../03-testing/00-integration-testing.md) for detailed guide.

## Key Dependencies

| Category | Crates | Purpose |
|----------|--------|---------|
| **Proxy core** | `pingora-core`, `pingora-proxy`, `pingora-http`, `pingora-load-balancing` | HTTP proxy engine |
| **Async** | `tokio`, `tokio-stream`, `futures`, `async-trait` | Async runtime |
| **gRPC** | `tonic`, `tonic-reflection`, `prost` | Controller ↔ Gateway communication |
| **HTTP API** | `axum`, `tower-http`, `hyper-util` | Admin APIs |
| **K8s** | `kube`, `k8s-openapi`, `schemars` | K8s integration + CRD schema |
| **Serialization** | `serde`, `serde_json`, `serde_yaml`, `toml` | Config parsing |
| **TLS** | `rustls`, `tokio-rustls`, `boring-sys` | TLS termination (rustls or BoringSSL) |
| **Observability** | `tracing`, `metrics` | Logging + metrics |
| **Security** | `jsonwebtoken`, `bcrypt`, `base64` | Auth plugins |
| **Networking** | `reqwest` | Plugin HTTP client (external calls) |
| **Performance** | `tikv-jemallocator`, `dashmap`, `arc-swap`, `smallvec` | Memory allocator, concurrent maps, lock-free reads, stack buffers |
