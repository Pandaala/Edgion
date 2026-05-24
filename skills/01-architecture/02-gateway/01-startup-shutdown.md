---
name: gateway-startup-shutdown
description: edgion-gateway startup and shutdown — 14-step startup sequence, two-phase Pingora integration, Tokio runtime lifecycle management.
---

# Gateway startup and shutdown

## Startup entry

`EdgionGatewayCli::run()` (`cli/mod.rs`) orchestrates the entire startup flow. The method runs synchronously on the `main()` thread, internally creates a Tokio Runtime for async operations, and finally hands control off to the Pingora main loop.

## 14-step startup sequence

```rust
// cli/mod.rs - EdgionGatewayCli::run()
pub fn run(&self) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    // ... 14 steps below ...
}
```

### Step 1: Load TOML configuration

```rust
let config = EdgionGatewayConfig::load(self.config.clone())?;
```

`EdgionGatewayConfig::load()` merges CLI arguments and the TOML configuration file. The configuration structure is defined in `cli/config.rs` and includes sections such as server, access_log, ssl_log, tcp_log, tls_log, udp_log, rate_limit, all_endpoint_status.

### Step 2: Initialize the working directory

```rust
init_work_dir(work_dir_path)?;
let wd = work_dir();
wd.validate()?;
```

Working directory priority: CLI `--work-dir` > environment variable `EDGION_WORK_DIR` > configuration file `work_dir` > current directory `"."`. `init_work_dir()` creates the necessary subdirectory structure and `validate()` checks writability.

### Step 3: Initialize the logging system

```rust
let log_config = config.to_log_config();
let _log_guard = runtime.block_on(init_logging(log_config))?;
```

`init_logging()` configures the tracing logging framework and returns a `WorkerGuard` to ensure logs are flushed when the process exits. It also initializes the RateLimit global configuration and the AllEndpointStatus global configuration.

### Step 4: Create ConfigSyncClient (gRPC to Controller)

```rust
let mut sync_client = runtime.block_on(Self::create_config_sync_client(&config))?;
let config_client = sync_client.get_config_client();
```

`ConfigSyncClient::new()` establishes a gRPC connection to the Controller. Parameters include `server_addr`, the client name `"edgion-gateway"`, and a 10-second timeout. `get_config_client()` returns `Arc<ConfigClient>`, aggregating ClientCache instances of all resource kinds.

### Step 5: Initialize the global ConfigClient

```rust
init_global_config_client(config_client.clone())?;
```

Registers the ConfigClient into a global `OnceLock<Arc<ConfigClient>>` so other modules can access it via `get_global_config_client()`.

### Step 6: Get ServerInfo from the Controller

```rust
let server_info = runtime.block_on(sync_client.get_server_info())?;
```

The `GetServerInfo` gRPC call returns:
- `supported_kinds`: list of resource kinds supported by the Controller, used for selective Watch.
- `server_id`: Controller instance identifier, used for version consistency checks.

It also initializes integration testing mode and the global testing mode (controlled by the `--integration-testing-mode` CLI flag).

### Step 7: Start watching resources

```rust
runtime.block_on(sync_client.start_watch_kinds(&server_info.supported_kinds))?;
```

Based on the `supported_kinds` list returned by the Controller, start a gRPC Watch stream for each resource kind. Also start the `start_watch_server_meta()` background task to monitor the gateway instance count for cluster-level rate limiting.

### Step 8: Start auxiliary services

```rust
runtime.block_on(Self::start_auxiliary_services(config_client.clone()));
```

`start_auxiliary_services()` starts four components:
1. **BackendCleaner**: `BackendCleaner::new().start()`, cleans up stale backend state for LeastConnection LB.
2. **Health Check Manager**: `get_hc_config_store()` + `get_health_check_manager()` initialize the health-check configuration store and manager.
3. **Admin API**: `tokio::spawn(api::serve(config_client, admin_listen))`, listens on the address from `gateway.admin_listen` (default `0.0.0.0:5810`).
4. **Metrics API**: `tokio::spawn(metrics::serve(5901))`, port 5901, exposes Prometheus metrics.

### Step 9: Wait for all caches to be ready

```rust
runtime.block_on(Self::wait_for_ready(config_client.clone()))?;
```

Polls `config_client.is_ready()` (once per second) until all ClientCache instances have completed their first List (`CacheData.is_ready() == true`). This ensures resources such as Gateway/GatewayClass/EdgionGatewayConfig/HTTPRoute are loaded before traffic is accepted.

### Step 10: Preload load balancers

```rust
crate::core::gateway::backends::preload_load_balancers(config_client.clone());
```

Iterates over all loaded routes and warms the LB selector (backend discovery + weight calculation) for each BackendRef, reducing first-request latency.

### Step 11: Initialize all observability loggers

```rust
runtime.block_on(init_access_logger(&config.access_log))?;
runtime.block_on(init_ssl_logger(&config.ssl_log))?;
runtime.block_on(init_tcp_logger(&config.tcp_log))?;
runtime.block_on(init_tls_logger(&config.tls_log))?;
runtime.block_on(init_udp_logger(&config.udp_log))?;
```

Five loggers handle HTTP access logs, SSL handshake logs, TCP connection logs, TLS route logs, and UDP logs respectively. Each logger configures its output target independently (file / stdout / LinkSys).

### Step 12: Create the Pingora server (Phase 1 — configure listeners)

```rust
let pingora_server = runtime.block_on(async {
    tokio::task::spawn_blocking(move || create_and_configure_server(config_client, &config))
        .await
        .expect("Failed to spawn blocking task")
})?;
```

Phase 1 runs within the Tokio Runtime context (`spawn_blocking` ensures UDP listeners can use a Tokio UDP socket):

1. **Create ServerConf**: read `threads` (default = CPU cores), `work_stealing` (default true), `grace_period_seconds` (default 30), `graceful_shutdown_timeout_seconds` (default 10), and `upstream_keepalive_pool_size` (default 128) from the TOML configuration.
2. **Bootstrap**: `Server::new_with_opt_and_conf(None, server_conf)` + `pingora_server.bootstrap()`.
3. **Create GatewayBase**: `GatewayBase::new(config_client)`.
4. **Get the Gateway list**: `config_client.list_gateways().data`.
5. **Configure listeners**: `gateway_base.configure_listeners(&mut pingora_server, gateways)`.

`configure_listeners()` iterates over each Listener of each Gateway and, in order:
- Looks up the associated GatewayClass and EdgionGatewayConfig.
- Checks for port conflicts and the Conflicted state.
- Parses the HTTP/2 annotation (`edgion.io/enable-http2`).
- Builds a `ListenerContext` and calls `listener_builder::add_listener()` to add the Listener to the Pingora Server.

### Step 13: Move the Tokio runtime to a background thread

```rust
std::thread::spawn(move || {
    runtime.block_on(async {
        std::future::pending::<()>().await;
    });
});
```

Moves ownership of the Tokio Runtime into a background thread. `std::future::pending()` keeps the thread alive permanently, ensuring all Tokio async tasks (gRPC Watch, Admin API, Metrics API, Health Check, etc.) keep running.

### Step 14: Run the Pingora server (Phase 2 — blocking main loop)

```rust
run_server(pingora_server);
// fn run_server(server: Server) { server.run_forever(); }
```

Phase 2 calls `server.run_forever()` on the main thread, starting the Pingora worker thread pool to accept connections and handle requests. This call blocks until a shutdown signal is received.

## Two-phase Pingora integration

| Phase | Function | Running thread | Responsibility |
|-------|----------|----------------|----------------|
| Phase 1 | `create_and_configure_server()` | Tokio Runtime (spawn_blocking) | Create ServerConf, bootstrap, configure all listeners |
| Phase 2 | `run_server()` → `server.run_forever()` | Main thread | Start Pingora worker thread pool, begin accepting connections |

Reason for the split: Phase 1 needs the Tokio context (UDP listener depends on a Tokio UDP socket), and Phase 2's `run_forever()` takes over thread control, so it must be invoked after the Tokio Runtime has been moved away.

## Shutdown flow

Shutdown relies on Pingora's built-in graceful shutdown mechanism:

1. Pingora receives SIGTERM/SIGINT.
2. Enter the grace period (default 30 seconds): existing connections continue to be served, `session.is_process_shutting_down()` returns true.
3. In `early_request_filter`, disable keepalive: `session.set_keepalive(None)`.
4. In `response_filter`, add the `Connection: close` response header.
5. After the grace period expires, enter the graceful shutdown timeout (default 10 seconds), forcibly closing remaining connections.
6. `run_forever()` returns, the main thread exits.
7. The background Tokio thread terminates automatically when the process exits.
