# 基于 Pingora 的数据面

> Gateway 启动流程、Pingora ProxyHttp 请求生命周期、ConnectionFilter TCP 层过滤。
> 数据面基于 Cloudflare Pingora 构建，支持 HTTP/1.1、HTTP/2、gRPC、TCP、UDP、TLS、WebSocket。

## Gateway Startup Sequence

```
1. Load config (EdgionGatewayConfig)
2. Create ConfigSyncClient → connect to controller gRPC
3. Fetch server info (endpoint mode, supported kinds)
4. Start watching all resource kinds from controller
5. Start auxiliary services (backend cleaner, admin API :5900, metrics :5901)
6. Wait until all caches ready
7. Preload load balancers
8. Initialize loggers (access, SSL, TCP, UDP)
9. Configure Pingora listeners via GatewayBase
10. Run Pingora server (blocks until shutdown)
```

## Pingora ProxyHttp — HTTP/gRPC Lifecycle

`EdgionHttp` implements `pingora_proxy::ProxyHttp` with `CTX = EdgionHttpContext`:

```
Client Request
  │
  ▼
early_request_filter()     ← ACME HTTP-01 challenge handling
  │
  ▼
request_filter()           ← Core: metadata extraction, route matching,
  │                          plugin chain (RequestFilter), XFF/X-Real-IP
  │                          Sets ctx.plugin_running_result
  ▼
upstream_peer()            ← Backend selection (HTTP vs gRPC), LB, timeout config
  │                          Checks plugin_running_result for early termination
  ▼
connected_to_upstream()    ← Connection established callback
  │
  ▼
upstream_response_filter() ← Sync: response plugins (UpstreamResponseFilter),
  │                          server header, status/timing recording
  ▼
upstream_response_body_filter() ← Sync per-chunk: bandwidth limiting
  │
  ▼
response_filter()          ← Async response processing
  │
  ▼
logging()                  ← Metrics update, AccessLogEntry build + send
```

**Key files:** `src/core/gateway/routes/http/proxy_http/pg_*.rs` (one file per hook)

## Connection Filter — TCP-Level (StreamPlugins)

Runs before TLS/HTTP, at raw TCP level:

```
TCP Connection arrives
  → ConnectionFilter.check(session)
    → StreamPluginConnectionFilter
      → StreamPluginStore.get(store_key)
      → StreamPluginRuntime.run(&StreamContext)
        → Each plugin: Allow or Deny(reason)
      → First Deny wins → reject connection
```

Configured per Gateway listener via annotation: `edgion.io/edgion-stream-plugins: "namespace/name"`.

**Key files:**
- `src/core/gateway/plugins/stream/connection_filter_bridge.rs`
- `src/core/gateway/plugins/stream/stream_plugin_runtime.rs`
- `src/core/gateway/runtime/server/listener_builder.rs` — `apply_connection_filter()`

## Access Log — High Efficiency Design

Goal: **one access log line captures all behavior/errors for a request**.

```
EdgionHttpContext (per-request, carried through entire lifecycle)
  │
  │  Contains:
  │  ├── request_info (client_addr, path, hostname, trace_id, ...)
  │  ├── edgion_status (error codes, warnings)
  │  ├── backend_context (service, upstream attempts, connect time)
  │  ├── stage_logs (Vec<StageLogs>: plugin execution logs per stage)
  │  ├── plugin_running_result (final plugin result)
  │  └── ctx_map (plugin-set variables)
  │
  ▼  At logging() hook:
AccessLogEntry::from_context(ctx)    ← Borrows from ctx, zero copy
  │
  ▼
entry.to_json()                      ← Single serde_json::to_string()
  │
  ▼
access_logger.send(json).await       ← Async, non-blocking
  │
  ▼
DataSender<String>                   ← Pluggable output via LinkSys
  ├── LocalFileWriter (default)        (queue + rotation)
  ├── Elasticsearch (future)
  └── Kafka (future)
```

**PluginLog budget:** Fixed 100-byte `SmallVec` buffer per plugin, stack-allocated (zero heap). Overflow tracked by `log_full` flag. Each plugin writes concise outcome strings: `"OK u=jack; "`, `"Deny ip=1.2.3.4; "`.

**Key files:**
- `src/types/ctx.rs` — `EdgionHttpContext`
- `src/core/gateway/observe/access_log/entry.rs` — `AccessLogEntry`
- `src/core/gateway/observe/access_log/logger.rs` — `AccessLogger`
- `src/core/gateway/observe/logs/logger_factory.rs` — `create_async_logger()`
- `src/core/gateway/plugins/runtime/log.rs` — `PluginLog`, `LogBuffer` (100-byte SmallVec)

## LinkSys Design

LinkSys is a CRD for declaring external system connections:

```yaml
apiVersion: edgion.io/v1alpha1
kind: LinkSys
spec:
  system:
    redis:
      endpoints: [...]
    # or: etcd, elasticsearch, kafka
```

**`SystemConfig` variants:** `Redis`, `Etcd`, `Elasticsearch`, `Kafka`

**Core abstraction:** `DataSender<T>` trait — async send to any backend. Currently implemented:
- `LocalFileWriter` — file output with rotation (for access/TCP/UDP/SSL logs)
- Future: ES, Kafka via LinkSys config

**Usage:** Observability sinks (access log, TCP log, UDP log, SSL log), rate limit state (future: Redis-backed).

**Key files:**
- `src/types/resources/link_sys/` — CRD type definitions
- `src/core/gateway/link_sys/runtime/` — `DataSender`, `LinkSysStore`, `ConfHandler`
- `src/core/gateway/link_sys/providers/local_file/` — `LocalFileWriter`
- `src/types/output.rs` — `StringOutput` (local file vs external)
