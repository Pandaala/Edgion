# LinkSys Development Guide

> Quick reference for adding new LinkSys external system connectors. Choose the best Rust crate for each system, design the interface around its native API, and follow this guide to integrate cleanly.
>
> **TODO (2026-02-25): Small Improvement**
> - [ ] Add detailed `ConfHandler` bridge pattern code snippet (`full_set()` and `partial_update()` implementation)
> - [ ] Add Webhook subsystem documentation (currently listed in directory but not described)

## Architecture Overview

LinkSys has two layers:

1. **CRD Layer** (`types/resources/link_sys/`) — Declares connections to external systems (Redis, Etcd, ES, Kafka). Managed as K8s CRDs, synced from controller → gateway via gRPC.
2. **Runtime Layer** (`core/gateway/link_sys/`) — Provides the `DataSender<T>` trait, `LinkSysStore`, and concrete client implementations that actually connect to external systems at runtime.

```
LinkSys CRD (config declaration)
    ↓ synced via gRPC
ClientCache<LinkSys> on gateway
    ↓ watch events (add/update/delete)
LinkSysConfHandler (ConfHandler impl, in runtime/conf_handler.rs)
    ↓ invokes LinkSysStore.replace_all / update
LinkSysStore (runtime/store.rs)
    ↓ dispatch_full_set / dispatch_partial_update
    ↓ match SystemConfig type → build/swap/shutdown runtime clients
Typed runtime stores (e.g., REDIS_RUNTIME, ETCD_RUNTIME)
    ↓ get_redis_client("namespace/name") / get_etcd_client("namespace/name")
Concrete client (e.g., RedisLinkClient, EtcdLinkClient)
    ↓ used by
Plugins, DataSender impls, etc.
```

**No separate store needed in user code.** The `ConfHandler` pattern (same as `PluginStore` for `EdgionPlugins`) ties the runtime client lifecycle directly to the CRD lifecycle. When a LinkSys CRD is created, the handler builds the client; when updated, it swaps; when deleted, it shuts down. Callers just call `get_redis_client("ns/name")`.

## Directory Structure

```
src/
├── core/gateway/link_sys/
│   ├── mod.rs                          # Re-exports: DataSender, LocalFileWriter, get_redis_client, etc.
│   ├── runtime/
│   │   ├── mod.rs                      # Runtime facade re-exports
│   │   ├── conf_handler.rs             # ConfHandler trait impl (bridges ClientCache → LinkSysStore)
│   │   ├── data_sender.rs              # DataSender<T> trait (async init/send/healthy/name)
│   │   └── store.rs                    # LinkSysStore + typed runtime stores + dispatch logic
│   ├── providers/
│   │   ├── local_file/                 # LocalFileWriter implementation (file + rotation)
│   │   ├── webhook/                    # Webhook subsystem
│   │   ├── redis/                      # Redis connector (implemented)
│   │   ├── etcd/                       # Etcd connector
│   │   ├── elasticsearch/              # Elasticsearch connector
│   │   └── <system>/                   # Future system connectors
├── types/
│   ├── resources/link_sys/
│   │   ├── mod.rs                      # LinkSys CRD, SystemConfig enum, validate_config()
│   │   ├── common.rs                   # SecretReference
│   │   ├── redis.rs                    # RedisClientConfig (CRD config types)
│   │   ├── etcd.rs                     # EtcdClientConfig (CRD config types)
│   │   └── <system>.rs                 # New system CRD config
│   └── ...
```

## Design Philosophy: Library-First

When adding a new LinkSys system, follow this principle:

> **Choose the best Rust crate for the job, then design the interface around its native API.**

This means:

1. **Pick the most popular, well-maintained crate** for the target system
2. **Don't invent a generic abstraction** that fights the library — let the library's strengths shine
3. **Expose high-level operations** that map naturally to the library's API
4. **Handle topology/deployment modes** via the library's built-in support (not custom wrappers)
5. **Version compatibility** — pin to a specific major version; most well-maintained crates have stable APIs across minor versions, so breakage risk is low

### Recommended Libraries

| System | Crate | Why |
|--------|-------|-----|
| **Redis** | [`fred`](https://crates.io/crates/fred) | Unified API for standalone/sentinel/cluster; built-in pool, reconnect, TLS, tracing; tokio-native |
| **Etcd** | [`etcd-client`](https://crates.io/crates/etcd-client) | Already in Cargo.toml; official-style async client |
| **Elasticsearch** | [`elasticsearch`](https://crates.io/crates/elasticsearch) | Official Elastic client for Rust |
| **Kafka** | [`rdkafka`](https://crates.io/crates/rdkafka) | Mature, librdkafka-based, async support |

## New LinkSys System Checklist

Adding a new system touches these areas:

### If CRD Config Types Already Exist (e.g., Redis, Etcd)

1. **Runtime client** — `src/core/gateway/link_sys/providers/<system>/client.rs`
2. **Config mapping** — `src/core/gateway/link_sys/providers/<system>/config_mapping.rs` (CRD → library config)
3. **Operations** — `src/core/gateway/link_sys/providers/<system>/ops.rs`
4. **DataSender** (optional) — `src/core/gateway/link_sys/providers/<system>/data_sender.rs` (only if used as log sink)
5. **Module exports** — `src/core/gateway/link_sys/providers/<system>/mod.rs` + `src/core/gateway/link_sys/providers/mod.rs` + `src/core/gateway/link_sys/mod.rs`
6. **Link store dispatch** — Add `SystemConfig::<System>` match branches in `src/core/gateway/link_sys/runtime/store.rs`
7. **Cargo.toml** — Add or update the library dependency with appropriate features
8. **Gateway Admin API testing endpoints** — Add testing endpoints in `src/core/gateway/api/mod.rs` under `create_testing_router()` for `--integration-testing-mode`
9. **Integration test config** — `examples/test/conf/LinkSys/<System>/` (CRD YAMLs + minimal Gateway resource)
10. **Docker Compose** — `examples/test/conf/Services/<system>/docker-compose.yaml` (external service for testing)
11. **Integration test script** — `examples/test/scripts/integration/run_<system>_test.sh`

### If CRD Config Types Don't Exist

Also add:

12. **CRD config** — `src/types/resources/link_sys/<system>.rs`
13. **SystemConfig variant** — Add to `SystemConfig` enum in `link_sys/mod.rs`
14. **Validation** — Add to `LinkSys::validate_config()`
15. **CRD YAML** — Update `config/crd/edgion-crd/link_sys_crd.yaml`

## Step-by-Step: Adding a Runtime Client

### Step 1: Add Dependency

In `Cargo.toml`, add the library with minimal required features:

```toml
# Example for Redis (fred)
fred = { version = "10", default-features = false, features = [
    "enable-rustls",      # TLS via rustls (consistent with Edgion)
    "i-std",              # Common data structures (GET, SET, lists, etc.)
    "i-keys",             # Key commands
    "i-hashes",           # Hash commands
    "i-lists",            # List commands
    "i-scripts",          # Lua scripting (for distributed locks)
    "partial-tracing",    # Tracing integration
] }

# Example for Etcd (already in Cargo.toml, add tls feature)
etcd-client = { version = "0.18", features = ["tls"] }
```

**Feature selection rules:**
- Only enable features you actually need (minimize compile time)
- Prefer `rustls` over `native-tls` for TLS (consistent with Edgion project)
- Enable `partial-tracing` or `metrics` only if observability integration is planned

### Step 2: Create Client Module

`src/core/gateway/link_sys/providers/<system>/client.rs`:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use anyhow::Result;

use crate::types::resources::link_sys::<system>::<System>ClientConfig;
use super::config_mapping::build_<system>_client;

/// Runtime client wrapper for <System>.
/// Built from LinkSys CRD config, managed by LinkSysStore (ConfHandler-driven).
pub struct <System>LinkClient {
    // Library-native client type (use Pool for libraries with pool support)
    inner: <library>::Client,
    // Human-readable name ("namespace/name")
    name: String,
    // Atomic health flag, updated by connection events or background monitor
    healthy: Arc<AtomicBool>,
}

impl <System>LinkClient {
    /// Create from CRD config. Does NOT connect — call init() next.
    pub fn from_config(name: &str, config: &<System>ClientConfig) -> Result<Self> {
        let inner = build_<system>_client(config)?;
        Ok(Self {
            inner,
            name: name.to_string(),
            healthy: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Initialize connection. Called once after construction.
    /// For libraries with event support, set up health tracking listeners here.
    pub async fn init(&self) -> Result<()> {
        // Connect, authenticate, verify
        // Set healthy = true on success
    }

    /// Check if the client is connected and responsive.
    #[inline]
    pub fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Get client name (namespace/name).
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Graceful shutdown — close connections, release resources.
    pub async fn shutdown(&self) -> Result<()> {
        // Close connections
        self.healthy.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Access the underlying library client for advanced operations.
    /// Prefer using the high-level operations in ops.rs when possible.
    #[inline]
    pub fn inner(&self) -> &<library>::Client {
        &self.inner
    }
}
```

**Key principles:**
- `from_config()` maps CRD types → library config (no library types in CRD layer)
- `init()` is separate from construction (allows lazy or deferred connection)
- `healthy()` is atomic, cheap, and safe for concurrent reads
- For libraries with built-in pool (e.g., fred), use the pool type directly
- For libraries without auto-reconnect (e.g., etcd-client), implement a background health monitor
- Expose the inner client for advanced use cases that ops.rs doesn't cover

### Step 3: Create Config Mapping Module

`src/core/gateway/link_sys/providers/<system>/config_mapping.rs`:

Isolate the CRD → library config mapping logic in its own file. This keeps `client.rs` focused on lifecycle management and makes the mapping logic independently testable.

```rust
use anyhow::{Context, Result};
use crate::types::resources::link_sys::<system>::<System>ClientConfig;

// Safety ceilings — prevent misconfigured CRDs from causing harm
const MAX_CONNECT_TIMEOUT_MS: u64 = 10_000;
const MAX_POOL_SIZE: usize = 64;
const DEFAULT_POOL_SIZE: usize = 8;

/// Map CRD config → library config.
pub fn build_<system>_config(crd: &<System>ClientConfig) -> Result<<library>::Config> {
    // Parse endpoints, auth, topology, TLS, timeouts
    // Apply safety ceilings (min/max bounds)
    // Return library-native config
}

/// URL parsing helpers (if the library needs host:port pairs)
pub(crate) fn parse_url(url: &str) -> Result<(String, u16)> {
    // Handle redis://, rediss://, http://, https://, plain host:port
}
```

**Design tips:**
- Always define safety ceilings (max pool size, max timeout, etc.)
- Add comprehensive unit tests for all config permutations
- Handle URL parsing robustly (IPv6, default ports, scheme stripping)

### Step 4: Create Operations Module

`src/core/gateway/link_sys/providers/<system>/ops.rs`:

Define high-level operations that plugins and other code will use. Don't wrap every library function — only expose what Edgion actually needs:

```rust
use std::time::Duration;
use anyhow::Result;
use serde::Serialize;
use super::client::<System>LinkClient;

// ============================================================================
// Domain-specific Operations (group logically)
// ============================================================================

impl <System>LinkClient {
    pub async fn get(&self, key: &str) -> Result<Option<String>> { /* ... */ }
    pub async fn set(&self, key: &str, value: &str, ttl: Option<Duration>) -> Result<()> { /* ... */ }
}

// ============================================================================
// Health Check (every system MUST have this)
// ============================================================================

/// Shared health status struct — used by admin API.
/// NOTE: This struct is defined in redis/ops.rs and re-exported.
/// Future systems should reuse it from link_sys_store or a shared module.
#[derive(Debug, Clone, Serialize)]
pub struct LinkSysHealth {
    pub name: String,
    pub system_type: String,      // "redis", "etcd", etc.
    pub connected: bool,
    pub latency_ms: Option<u64>,  // PING latency if connected
    pub error: Option<String>,    // Last error if unhealthy
}

impl <System>LinkClient {
    /// Active health check (PING or equivalent). Returns latency in ms.
    pub async fn ping(&self) -> Result<u64> { /* ... */ }

    /// Detailed health status for admin API.
    pub async fn health_status(&self) -> LinkSysHealth { /* ... */ }
}
```

**Design tips:**
- Group related operations logically (KV, Hash, List, Lock, Watch, Lease, etc.)
- Use standard Rust types in signatures (`String`, `Duration`, `Option<T>`, `HashMap`)
- Let the library handle serialization/deserialization internally
- Return `anyhow::Result` for consistency with `DataSender`
- Add doc comments explaining the underlying command being called
- **Every system MUST implement `ping()` and `health_status()`** — this is required for admin API health checks

### Step 5: Export and Register

`src/core/gateway/link_sys/providers/<system>/mod.rs`:

```rust
pub mod client;
pub mod config_mapping;
pub mod ops;
// pub mod data_sender;  // Only if DataSender is implemented

pub use client::<System>LinkClient;
// pub use data_sender::<System>DataSender;
// pub use ops::LinkSysHealth;  // Only if health struct is defined here
```

`src/core/gateway/link_sys/mod.rs` — add:

```rust
pub mod <system>;
pub use <system>::<System>LinkClient;
```

### Step 6: LinkSysStore — Add Dispatch Branches

In `src/core/gateway/link_sys/runtime/store.rs`, add:

#### 6a. Typed runtime store (ArcSwap)

```rust
use super::<system>::<System>LinkClient;

// Near REDIS_RUNTIME:
static <SYSTEM>_RUNTIME: LazyLock<ArcSwap<HashMap<String, Arc<<System>LinkClient>>>> =
    LazyLock::new(|| ArcSwap::from_pointee(HashMap::new()));

pub fn get_<system>_client(name: &str) -> Option<Arc<<System>LinkClient>> {
    <SYSTEM>_RUNTIME.load().get(name).cloned()
}

pub fn list_<system>_clients() -> Vec<String> {
    <SYSTEM>_RUNTIME.load().keys().cloned().collect()
}

// + insert/remove/replace_all helpers (same pattern as redis_runtime_*)
```

#### 6b. Dispatch branches in `dispatch_full_set` and `dispatch_partial_update`

```rust
// In dispatch_full_set, replace the placeholder:
crate::types::resources::link_sys::SystemConfig::Etcd(etcd_config) => {
    match <System>LinkClient::from_config(key, etcd_config) {
        Ok(client) => {
            let client = Arc::new(client);
            let client_ref = client.clone();
            let key_owned = key.clone();
            tokio::spawn(async move {
                if let Err(e) = client_ref.init().await {
                    tracing::error!(<system> = %key_owned, error = %e, "Failed to initialize <System> client");
                }
            });
            new_<system>_map.insert(key.clone(), client);
        }
        Err(e) => { tracing::error!(key, error = %e, "Failed to build <System> client from config"); }
    }
}
```

**Key pattern:** `init()` is always spawned in a background task to avoid blocking the sync path. The client is stored immediately (even before init completes) so `get_<system>_client()` returns it right away. On delete, `shutdown()` is also spawned in background.

### Step 7: Gateway Admin API Testing Endpoints

When `--integration-testing-mode` is enabled, the Gateway exposes admin API endpoints for testing LinkSys clients. Add endpoints in `src/core/gateway/api/mod.rs` under `create_testing_router()`.

**URL convention:** `/api/v1/testing/link-sys/<system>/{name}/{operation}`

Where `{name}` uses underscore as namespace separator: `"default_redis-test"` → key `"default/redis-test"`.

```rust
// In create_testing_router():
.route("/api/v1/testing/link-sys/<system>/health", get(<system>_health_all))
.route("/api/v1/testing/link-sys/<system>/clients", get(<system>_clients))
.route("/api/v1/testing/link-sys/<system>/{name}/health", get(<system>_health_one))
.route("/api/v1/testing/link-sys/<system>/{name}/ping", get(<system>_ping))
// ... system-specific operation endpoints ...
```

**Response format:** All endpoints return `ApiResponse<T>` (JSON: `{"success": bool, "data": T, "error": string}`).

**Helper pattern:**

```rust
fn get_<system>_client_by_name(name: &str) -> Result<Arc<<System>LinkClient>, Json<ApiResponse<serde_json::Value>>> {
    // URL path uses underscore as separator: "default_my-client" → "default/my-client"
    let key = name.replacen('_', "/", 1);
    crate::core::gateway::link_sys::get_<system>_client(&key).ok_or_else(|| {
        Json(ApiResponse::error(format!(
            "<System> client '{}' not found. Available: {:?}",
            key,
            crate::core::gateway::link_sys::runtime::store::list_<system>_clients()
        )))
    })
}
```

### Step 8: Integration Test Script + Config

Each LinkSys system has its own **independent integration test script** that uses **bash + curl + jq** to test through the Gateway Admin API (not cargo test). This approach tests the full stack: Docker service → Controller → Gateway → Runtime Client.

#### 8a. Test Configuration Files

```
examples/test/conf/
├── LinkSys/<System>/
│   ├── 00_Gateway_<ns>_<system>-test-gateway.yaml   # Minimal Gateway resource (required for Gateway startup)
│   └── 01_LinkSys_<ns>_<system>-test.yaml            # LinkSys CRD for the system
├── Services/<system>/
│   └── docker-compose.yaml                            # Docker service for testing
```

**Important:** The Gateway process requires at least one Gateway CRD to start. Even if LinkSys tests don't route traffic, include a minimal Gateway resource in the test config.

#### 8b. Test Script Structure

`examples/test/scripts/integration/run_<system>_test.sh`:

```bash
#!/bin/bash
# Self-contained integration test for <System> LinkSys.
# 1. Cleanup old processes (pkill -9 -f edgion-controller/gateway)
# 2. Start external system (docker compose up)
# 3. Start Controller with --conf-dir pointing to temp work directory
# 4. Load base configs (GatewayClass, EdgionGatewayConfig, etc.)
# 5. Load LinkSys configs (Gateway resource + LinkSys CRD)
# 6. Start Gateway with --integration-testing-mode
# 7. Run tests via curl + jq against Gateway Admin API
# 8. Cleanup (kill processes, docker compose down)
```

**Critical patterns** (learned from Redis implementation):
- Use `curl -s` (NOT `curl -sf`) — `-f` suppresses error response bodies
- Use `jq 'if .success == false then "false" else "true" end'` — `jq`'s `//` operator treats `false` same as `null`
- Always wait for Gateway LB preload (`/api/v1/upstream_info`) before running tests
- Controller `--conf-dir` must point to a temp work directory (not `examples/test/conf/`) to avoid polluting the source config directory with FileSystemWriter output
- Use `pkill -9 -f` for robust process cleanup at both start and end of script

**Test categories per system:**
- Resource sync (CRD synced to Gateway)
- Client registration (client appears in runtime store)
- Health & PING (connectivity verification)
- System-specific operations (CRUD, lock, watch, etc.)
- Error handling (non-existent client, invalid operations)
- Lifecycle (CRD delete → client removed)

## CRD Config ↔ Runtime Client Mapping

The CRD config types (`types/resources/link_sys/<system>.rs`) define what users configure in YAML. The config mapping (`core/gateway/link_sys/providers/<system>/config_mapping.rs`) translates this into library-specific config.

```
User YAML → CRD Type (serde) → config_mapping.rs → Library Config → Library Client
```

**Important:** CRD types must be serializable (serde + JsonSchema). They should NOT contain library-specific types. The `config_mapping.rs` module bridges the two worlds.

Example mapping (Redis):

| CRD Config | Library Config |
|------------|---------------|
| `endpoints: ["redis://..."]` | `fred::Config::from_url()` or `ServerConfig` |
| `topology.mode: cluster` | `fred::Config { server: ServerConfig::Cluster { ... } }` |
| `pool.size: 10` | `fred::Builder::set_pool_size(10)` |
| `auth.password` | `fred::Config { password: Some(...) }` |
| `tls.enabled: true` | `fred::Builder::with_tls_config(...)` |
| `timeout.connect: 5000` | `fred::Builder::with_connection_config(\|c\| c.connection_timeout = ...)` |

Example mapping (Etcd):

| CRD Config | Library Config |
|------------|---------------|
| `endpoints: ["http://..."]` | `Client::connect(endpoints, options)` |
| `auth.username/password` | `ConnectOptions::with_user(user, pass)` |
| `tls.enabled: true` | `ConnectOptions::with_tls(TlsOptions)` |
| `timeout.dial: 5000` | `ConnectOptions::with_connect_timeout(Duration)` |
| `keepAlive.time/timeout` | `ConnectOptions::with_keep_alive(time, timeout)` |
| `namespace: "/app/"` | Key prefix applied in ops (not a native ConnectOptions feature) |

## Health Check Pattern

Every LinkSys client **MUST** support health checking for observability:

```rust
/// Health status of a LinkSys client, exposed via admin API.
#[derive(Debug, Clone, Serialize)]
pub struct LinkSysHealth {
    pub name: String,
    pub system_type: String,      // "redis", "etcd", etc.
    pub connected: bool,
    pub latency_ms: Option<u64>,  // PING latency if connected
    pub error: Option<String>,    // Last error message if unhealthy
}
```

Health check should be:
- **Non-blocking** — use the library's built-in PING or equivalent
- **Cached** — don't PING on every call; update periodically or on connection events
- **Surfaced via admin API** — both production (`/api/v1/link-sys/health`) and testing endpoints

**Implementation notes:**
- For libraries with event listeners (fred), use `on_reconnect`/`on_error` to update `AtomicBool`
- For libraries without events (etcd-client), use a background health monitor task with exponential backoff
- `ping()` does an active check (sends PING/status command); `healthy()` returns the cached `AtomicBool`

## DataSender Integration

If the system is used as a log sink (ES, Kafka, Redis as failed cache), implement `DataSender<T>` in a separate `data_sender.rs` file:

```rust
#[async_trait]
impl DataSender<String> for <System>DataSender {
    async fn init(&mut self) -> Result<()> { /* ... */ }
    fn healthy(&self) -> bool { /* ... */ }
    async fn send(&self, data: String) -> Result<()> {
        // System-specific write logic
    }
    fn name(&self) -> &str { &self.name }
}
```

Not all systems need `DataSender` — Redis used for rate limiting or distributed locks doesn't need it. Only implement when the system is used as an output sink.

## Secret Handling

Credentials in CRD configs reference K8s Secrets via `SecretReference`:

```rust
pub struct SecretReference {
    pub name: String,
    pub namespace: Option<String>,
    pub key: Option<String>,
}
```

At parse time (controller side), resolve the secret:

```rust
use crate::core::controller::conf_mgr::sync_runtime::resource_processor::get_secret;

if let Some(secret_ref) = &config.auth.secret_ref {
    let secret = get_secret(Some(namespace), &secret_ref.name);
    // Extract password/cert from secret data
}
```

At runtime (gateway side), secrets are already resolved into `resolved_*` fields.

## Testing

### Unit Tests

Each module should have `#[cfg(test)]` tests:

- **config_mapping.rs** — Test all config permutations: standalone/cluster/sentinel, auth, TLS, timeouts, safety ceilings, URL parsing
- **ops.rs** — Test utility functions (e.g., lock value generation)
- **client.rs** — Test `from_config` with minimal/full configs, error cases

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_config_mapping_minimal() {
        let crd = minimal_config(vec!["redis://localhost:6379".to_string()]);
        let config = build_fred_config(&crd).unwrap();
        assert!(config.server.is_centralized());
    }

    #[test]
    fn test_pool_size_clamped_to_max() {
        let crd = RedisClientConfig { pool: Some(RedisPool { size: Some(200), .. }), .. };
        let (_, pool_size) = build_fred_pool(&crd, config).unwrap();
        assert_eq!(pool_size, MAX_POOL_SIZE);  // Clamped to 64
    }

    #[test]
    fn test_empty_endpoints_returns_error() {
        let crd = minimal_config(vec![]);
        assert!(build_fred_config(&crd).is_err());
    }
}
```

### Integration Tests — Independent Test Scripts

Each LinkSys system has its own **independent integration test script** in `examples/test/scripts/integration/`, following the same pattern as `run_acme_test.sh`:

```
examples/test/scripts/integration/
├── run_acme_test.sh              # ACME tests (existing)
├── run_redis_test.sh             # Redis LinkSys integration tests
├── run_etcd_test.sh              # Etcd LinkSys integration tests
└── ...
```

**How it works:** The test script starts the full stack (Docker service + Controller + Gateway with `--integration-testing-mode`), then uses `curl` + `jq` to call the Gateway Admin API testing endpoints. This tests the complete path: CRD config → ConfHandler → runtime client → actual operations against the real external service.

**Script supports:**
- `--no-cleanup`: keep containers and processes running for debugging
- `--no-build`: skip `cargo build` (use existing binaries)
- Colored output: `[INFO]`, `[✓]`, `[✗]` for easy reading
- Assertion helpers: `assert_json_success`, `assert_json_failure`, `assert_eq`
- Test summary: total/passed/failed count at the end

**Test scenarios** (per system):
- Connection (standalone / cluster / sentinel)
- Authentication (password, ACL, wrong password)
- TLS (encrypted connection)
- Operations (GET/SET, HGET/HSET, distributed lock, watch, lease, etc.)
- Health check (PING latency, connected status)
- Error handling (non-existent client, invalid parameters)
- ConfHandler lifecycle (CRD create → client created, CRD delete → client shutdown)

## Coding Principles

1. **Comments in English** — all code comments, doc comments, and log messages
2. **Library-first design** — don't fight the library's API; wrap minimally
3. **Fail loudly on init, recover silently at runtime** — startup failures should be clear; runtime disconnects should auto-reconnect
4. **No secrets in logs** — use `tracing` for operational logs; never log passwords, tokens, or connection strings with credentials
5. **Feature-gate heavy dependencies** — if the library is large (e.g., rdkafka), consider a Cargo feature to make it optional
6. **Background init/shutdown** — `init()` and `shutdown()` are spawned in background tasks in `runtime/store.rs` to avoid blocking the config sync path
7. **Safety ceilings** — Always clamp user-configurable values (pool size, timeouts) to safe maximums to prevent misconfigured CRDs from causing harm
