---
name: gateway-overview
description: edgion-gateway overall architecture — dual runtime model, configuration sync pipeline, Pingora proxy layer, module layout, key data flows.
---

# Gateway overall architecture

edgion-gateway is Edgion's data plane, a high-performance reverse proxy built on Cloudflare Pingora. It receives Kubernetes-style resource configuration from the Controller via gRPC and translates it into runtime proxy behavior.

## Canonical SSE substrate

`edgion-gateway/src/sse/` is the only production SSE framing and semantic
window substrate. `SseDecoder` preserves ordered raw records while exposing
callback-borrowed semantic event views; `sse::window::RawWindow` is the stable,
policy-neutral owned seam for bounded hold-and-release consumers. A consumer
must not retain a borrowed event across an `await`. It transfers raw slices and
stable fragment ranges into `RawWindow`, then consumes one explicit
release/replace/discard decision per generation and one final decision at EOS.

The window independently bounds logical raw bytes, visible resident capacity,
slice handles, ranges, lines, complete events, metadata, logical buckets,
fragments, semantic snapshots, replacement bytes, global overlap, and combined
residency. Handle pressure coalesces once per generation; other limit failures
are atomic and never silently truncate raw bytes. The bucket key is choice
index plus `Content`, `ReasoningContent`, or `ToolArguments(tool_call_index)`.

ExtProc response STREAMED remains transport-chunk based. Semantic-window
integration requires explicit configuration plus a reviewed source-aware
wrapper or driver contract; placing an ordinary processor before the current
ExtProc processor is not sufficient because sink extras are invisible to later
processors.

## Dual runtime model

Two runtimes coexist within the Gateway process:

| Runtime | Responsibility | Lifecycle |
|---------|----------------|-----------|
| **Tokio** | gRPC Watch/List, configuration handling, auxiliary services (Admin API, Metrics API, health checks), all async tasks | Created at startup, moved to a shutdown-aware background thread, drained and joined after Pingora stops |
| **Pingora** | Proxy main loop: TCP connection accept, TLS handshake, HTTP/gRPC request handling, upstream connection reuse | Runs blocking on the main thread via `Server::run_forever()` |

The two runtimes share configuration data via an **ArcSwap** atomic pointer — the Tokio side writes, the Pingora side reads lock-free. This design ensures that hot configuration updates do not block proxy request handling.

## Configuration sync pipeline

```mermaid
flowchart TD
    A["Controller gRPC"] --> B["ConfigSyncClient"]
    B --> C["ClientCache&lt;T&gt; (per resource kind)"]
    C -->|"event_dispatch (apply_change / set_ready)"| D["ConfHandler&lt;T&gt; (full_set / partial_update)"]
    D --> E["Runtime Stores (ArcSwap atomic swap)"]
```

Key components:

- **ConfigSyncClient** (`conf_sync/conf_client/grpc_client.rs`): manages the gRPC connection to the Controller, handles List/Watch calls, version tracking, and reconnection. Calls `get_server_info()` to obtain server_id and supported_kinds.
- **ClientCache\<T\>** (`conf_sync/cache_client/cache.rs`): one cache instance per resource kind, holding `CacheData<T>` (RwLock-protected) and a gRPC client reference. Implements the `DynClientCache` trait to enable type-erased unified dispatching.
- **event_dispatch** (`conf_sync/cache_client/event_dispatch.rs`): implements the `CacheEventDispatch<T>` trait, applying gRPC events (InitAdd/EventAdd/EventUpdate/EventDelete) to CacheData and compressing event batches.
- **ConfHandler\<T\>** (`edgion-common/src/conf_sync/traits.rs`): a resource-handler trait, one implementation per resource kind. Provides two callbacks, `full_set()` and `partial_update(add, update, remove)`, responsible for writing resources into runtime stores.
- **ConfigClient** (`conf_sync/conf_client/config_client.rs`): aggregates ClientCache instances of all resource kinds and exposes type-safe query interfaces such as `list_gateways()`, `get_gateway_class()`, `get_edgion_gateway_config()`, plus an `is_ready()` readiness check.

## Pingora proxy layer

### ConnectionFilter (TCP layer)

`StreamPluginConnectionFilter` implements Pingora's `ConnectionFilter` trait, filtering raw TCP connections before TLS handshake and HTTP parsing. It references the `EdgionStreamPlugins` resource via the Gateway annotation `edgion.io/edgion-stream-plugins` and reads the latest configuration from the global `StreamPluginStore` (ArcSwap), supporting hot reload. Each connection invokes `should_accept(addr)` to execute all StreamPlugin chains.

### ProxyHttp (HTTP/gRPC layer)

`EdgionHttpProxy` implements Pingora's `ProxyHttp` trait and handles the full lifecycle of HTTP and gRPC requests. With `EdgionHttpContext` as the per-request state carrier, requests pass through eight phases: early_request_filter → request_filter → upstream_peer → connected_to_upstream → upstream_response_filter → upstream_response_body_filter → response_filter → logging.

### TCP/TLS/UDP routes

Beyond HTTP/gRPC, the Gateway also supports:
- **TCPRoute**: TCP layer-4 proxy, matched per-listener (first-match, no SNI/hostname dimension).
- **TLSRoute**: SNI-based layer-4 proxy; supports Terminate (TLS→TCP) and Passthrough (encrypted forward) modes.
- **UDPRoute**: UDP proxy.

Each protocol has an independent `routes_mgr` (route manager) and `route_table`, both implemented with ArcSwap for lock-free hot updates.

## Module layout

```
edgion-gateway/src/
├── api/           # Admin API server (default :12001, configurable via admin_listen), exposes runtime state queries
├── backends/      # Backend management
│   ├── discovery/ # Backend discovery: EndpointSlice / Service stores
│   ├── health/    # Health checks, status store, manager scheduling
│   ├── policy/    # BackendTLSPolicy + EdgionBackendTrafficPolicy
│   ├── preload.rs # Hydrate referenced Service backend lists at startup
│   └── validation.rs
├── cache/         # Retained LRU plus OIDC introspection TinyUFO TTL cache
├── cli/           # Startup entry (EdgionGatewayCli::run())
│   ├── config.rs  # EdgionGatewayConfig YAML configuration structure
│   └── pingora.rs # Pingora Server creation and run (two-phase)
├── conf_sync/     # Configuration sync
│   ├── cache_client/ # ClientCache<T>, CacheData, event_dispatch
│   └── conf_client/  # ConfigSyncClient (gRPC), ConfigClient (aggregated queries)
├── config/        # Resource configuration handlers
│   ├── edgion_gateway/ # EdgionGatewayConfig ConfHandler
│   └── gateway_class/  # GatewayClass ConfHandler
├── lb/            # Load balancing
│   ├── backend_selector/ # Backend selector (weighted round-robin)
│   ├── selection/        # Strategy implementations: ConsistentHash, EWMA, LeastConn
│   ├── ewma/             # EWMA metric collection
│   ├── leastconn/        # Least-connection state and cleaner
│   ├── compiled_selector.rs # per-Service compiled selector cells
│   └── runtime_state/    # Runtime LB state (increment/decrement/update_ewma)
├── link_sys/      # Gateway LinkSys providers/runtime; common core + Etcd live in edgion-common
│   ├── providers/ # Elasticsearch, Kafka, Redis, Webhook, LocalFile, OTLP
│   └── runtime/   # ConfHandler, DataSender
├── observe/       # Observability
│   ├── access_log/  # AccessLog: entry definition, logger, store
│   └── metrics/     # Prometheus metrics, global counters
│   └── logs/        # System logs, SSL/TCP/TLS/UDP logs
├── plugins/       # Plugin system
│   ├── http/      # HTTP plugin set
│   ├── stream/    # Stream plugins (ConnectionFilter bridge, TLS route plugins)
│   └── runtime/   # Plugin execution engine (PluginRuntime, SessionAdapter)
├── routes/        # Multi-protocol routing
│   ├── http/      # HTTPRoute: matching engine (radix + regex), route manager, ProxyHttp impl
│   ├── grpc/      # GRPCRoute: matching and upstream handling
│   ├── tcp/       # TCPRoute
│   ├── tls/       # TLSRoute
│   └── udp/       # UDPRoute
├── runtime/       # Pingora runtime
│   ├── server/    # GatewayBase, listener_builder, error_response, server_header
│   ├── matching/  # Gateway matching, TLS certificate matching (ArcSwap)
│   ├── store/     # GatewayStore, ListenerGatewayInfoStore (ArcSwap)
│   ├── handler.rs # ConfHandler implementation for the Gateway resource
│   └── gateway_info.rs # compiled Gateway/listener runtime information
├── services/      # Auxiliary services
│   └── acme/      # ACME automatic certificate issuance (challenge store)
└── tls/           # TLS certificate management
    ├── boringssl/ # BoringSSL custom callback support
    ├── runtime/   # TLS callbacks (SNI matching, certificate selection)
    ├── store/     # Certificate storage
    └── validation/ # Certificate validation
```

## Key data flows

### Configuration update flow

```
Controller pushes resource changes
  → ConfigSyncClient receives gRPC Watch event
    → ClientCache<T>.apply_change() updates CacheData
      → Event compression (compress_events)
        → ConfHandler<T>.full_set() or partial_update()
          → Writes to runtime store (ArcSwap.store())
            → Pingora proxy side picks it up on next read (lock-free)
```

### Request handling flow

```
Client TCP connection
  → ConnectionFilter: StreamPlugins filtering (IP restrictions, etc.)
    → TLS handshake: SNI-based certificate match (GatewayTlsMatcher via ArcSwap)
      → HTTP parsing
        → early_request_filter: timeout setup, keepalive management
          → request_filter: metadata extraction, ACME challenge, route matching, plugin execution
            → upstream_peer: backend selection, LB strategy
              → Proxy request to upstream
                → upstream_response_filter/body_filter: response handling
                  → response_filter: async response plugins
                    → logging: metric recording + AccessLog
```
