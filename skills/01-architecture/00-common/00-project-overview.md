---
name: project-overview
description: Edgion project high-level architecture diagram, workspace crate structure, in-repo binary definitions, code organization, EdgionHttpContext, testing infrastructure, key dependencies.
---

# Project overview

## High-level architecture

```mermaid
flowchart TD
    YAML["YAML / K8s CRD"] --> CC
    CLI["edgion-cli"] --> AdminCtrl
    WasmLib["edgion-wasm"] --> CLI
    WafLib["edgion-waf"] --> RP

    subgraph Controller["edgion-controller"]
        CC["ConfCenter\n(File/K8s/Etcd)"] --> WQ["Workqueue\n(per-kind)"] --> RP["ResourceProcessor\n(validate/preparse/parse)"]
        AdminCtrl["Admin API (:12101)"]
        ProbeCtrl["Probe (:12100)"]
        MetricsCtrl["Metrics (:12190)"]
        SyncServer["ConfigSyncServer\n(gRPC :12151)"]
    end

    SyncServer -- "gRPC Watch/List" --> SyncClient

    subgraph Gateway["edgion-gateway"]
        SyncClient["ConfigSyncClient"] --> ClientCache["ClientCache\n(per-kind)"] --> Preparse["Preparse"]
        WasmLoad["Wasm compatibility + load"]
        subgraph Pingora["Pingora Server"]
            ConnFilter["ConnectionFilter\n(TCP-level, StreamPlugins)"]
            ProxyHttp["ProxyHttp (HTTP/gRPC lifecycle)"]
            ReqFilter["request_filter → route match + plugins"]
            UpstreamPeer["upstream_peer → backend selection + LB"]
            UpstreamResp["upstream_response → response plugins"]
            Logging["logging → AccessLog"]
            ProxyHttp --- ReqFilter
            ProxyHttp --- UpstreamPeer
            ProxyHttp --- UpstreamResp
            ProxyHttp --- Logging
            StreamRoutes["TCP/UDP/TLS Routes"]
        end
        AdminGw["Admin API (:12001 default)"]
        MetricsGw["Metrics API (:12090)"]
    end
    WasmLib --> WasmLoad
    WafLib --> ProxyHttp
```

## Crate structure

Cargo workspace with crate-owned binary entry points:

| Binary | Entry file | Runtime model | Role |
|--------|---------|-----------|------|
| `edgion-gateway` | `edgion-gateway/src/bin/edgion_gateway.rs` | Synchronous entry; internally manages its own Tokio + Pingora main loop | Data plane |
| `edgion-controller` | `edgion-controller/src/bin/edgion_controller.rs` | `#[tokio::main(multi_thread)]` | Control plane |
| `edgion-cli` | `edgion-cli/src/bin/edgion_ctl.rs` | `#[tokio::main]` | CLI tool |

> The federation Center binary (`edgion-center`) has been extracted to the separate EdgionCenter repository.

Example binaries (used in testing): `test_server`, `test_client`, `test_client_direct`, `resource_diff`, `config_load_validator`.

The Gateway defaults to `allocator-jemalloc` + `boringssl` and always builds
against the Edgion Pingora fork.
The Controller and CLI default only to `allocator-jemalloc`. Library crates do
not share those binary-feature defaults.

## Code directory organization

```
edgion-waf/                      # Native WAF parsing, semantics, and matching (leaf crate)
edgion-wasm/                     # Proxy-Wasm compatibility/profile analysis (leaf crate)
edgion-ldap/                     # Bounded LDAP bind client: BER codec + bind session (leaf crate)
edgion-dsl/                      # DSL parser, compiler, and static validator (leaf crate)
edgion-resources/                # Shared resource/config schemas and validation helpers
edgion-common/                   # Shared infra: config, observability, secret store, link system, proto codegen
edgion-controller/               # Control plane binary and library
edgion-gateway/                  # Data plane binary and library
edgion-cli/                      # CLI binary and library
edgion-tests/                    # Integration test assets, scripts, and validators
```

**Design principles**: `edgion-waf` is a runtime-independent leaf that owns the
native WAF engine without linking another WAF implementation. Its strict
compiler is consumed by `edgion-resources` for control/data-plane validation
parity, and its transaction runtime is consumed directly by the Gateway.
`edgion-wasm` is an external-dependencies-only leaf
consumed by the Gateway before Wasmtime compilation and by the CLI for local
Wasm inspection. `edgion-ldap` is a runtime-light leaf (as opposed to
runtime-free): besides `thiserror` it depends only on tokio's
`AsyncRead`/`AsyncWrite` traits, so its bind session runs over any stream the
Gateway has already dialed, admitted and wrapped in TLS; its sans-IO codec has
no recursion and a fixed frame cap. `edgion-dsl` is a leaf and
`edgion-resources` depends on it and `edgion-waf` for real DSL/WAF compilation and static validation;
`edgion-common` holds backend-agnostic shared infrastructure; controller,
gateway, and CLI code stay in their owning crates. The in-repo dependency graph
remains a DAG (`{dsl, waf} → resources → common → {controller, gateway, cli}`,
plus direct `waf → gateway`, `ldap → gateway` and `wasm → {gateway, cli}` edges); the Gateway also uses `edgion-dsl`
directly for VM/runtime integration.

## EdgionHttpContext — per-request state container

Defined in `edgion-gateway/src/ctx.rs`, the "backpack" carried through the entire HTTP request lifecycle:

| Field | Purpose |
|------|------|
| `start_time` | Request timing |
| `gateway_info` | Gateway metadata |
| `request_info` | Client address, hostname, path, trace ID, SNI, gRPC metadata |
| `diagnostics` | Bounded, ordered information/error diagnostics accumulated during request processing |
| `route_unit` / `grpc_route_unit` | The matched routing rule (containing `PluginRuntime`) |
| `selected_backend` / `selected_grpc_backend` | The selected backend reference |
| `backend_context` | Service name, upstream attempt count, connection time |
| `stage_logs` | `Vec<StageLogs>` — plugin logs per execution phase |
| `plugin_running_result` | Current plugin chain execution result |
| `ctx_map` | `HashMap<String, String>` — variables set by plugins |
| `parsed_request_caches` | Query parameters decoded on demand and path parameters frozen at route match |
| `hash_key` | Consistent-hashing key |
| `try_cnt` | Upstream connection attempt counter |

Created in `new_ctx()` and consumed in `logging()`. Plugins interact with it through the `PluginSession` adapter.

## Testing infrastructure

| Component | Path | Purpose |
|------|------|------|
| `test_server` | `edgion-tests/src/server/test_server.rs` | Multi-protocol echo backend (HTTP, gRPC, WebSocket, TCP, UDP, auth) |
| `test_client` | `edgion-tests/src/client/test_client.rs` | Test runner based on the TestSuite trait |
| `resource_diff` | `edgion-tests/src/validator/resource_diff.rs` | Controller ↔ Gateway sync verification |
| `run_integration.sh` | `edgion-tests/integration/scripts/integration/` | End-to-end integration test orchestration |
| Test configurations | `edgion-tests/integration/conf/` | YAML resources organized by `Resource/Item/` |
| Port registry | `edgion-tests/integration/conf/ports.json` | Unique port allocation per test suite |

## Key dependencies

| Category | Crate | Purpose |
|------|-------|------|
| **Proxy core** | `pingora-core`, `pingora-proxy`, `pingora-http`, `pingora-load-balancing` | HTTP proxy engine |
| **Async** | `tokio`, `tokio-stream`, `futures`, `async-trait` | Async runtime |
| **gRPC** | `tonic`, `prost` | Controller ↔ Gateway communication |
| **HTTP API** | `axum`, `tower`, `hyper-util` | Admin API |
| **K8s** | `kube`, `k8s-openapi`, `schemars` | K8s integration + CRD Schema |
| **Serialization** | `serde`, `serde_json`, `yaml_serde` | Configuration parsing |
| **TLS** | `rustls`, `tokio-rustls`, `boring-sys` | BoringSSL Gateway termination; rustls-based backend-independent clients and links |
| **Observability** | `tracing`, `metrics` | Logging + metrics |
| **Security** | `jsonwebtoken`, `base64`, `sha1`, `sha2` | Controller admin auth is token-only (SHA-256 token-hash compare, no password hashing); gateway plugins use `jsonwebtoken` (jwt_auth), `sha1`/`md5` (basic_auth htpasswd), `base64` (everywhere) |
| **Performance** | `tikv-jemallocator`, `dashmap`, `arc-swap`, `smallvec` | Memory allocator, concurrent map, lock-free reads, stack buffers |
