# LinkSys Development Guide

> Quick reference for adding new LinkSys external system connectors. Choose the best Rust crate for each system, design the interface around its native API, and follow this guide to integrate cleanly.

## Architecture Overview

LinkSys has two layers:

1. **CRD Layer** (`types/resources/link_sys/`) — Declares connections to external systems (Redis, Etcd, ES, Kafka). Managed as K8s CRDs, synced from controller → gateway via gRPC.
2. **Runtime Layer** (`core/link_sys/`) — Provides the `DataSender<T>` trait and concrete client implementations that actually connect to external systems at runtime.

```
LinkSys CRD (config declaration)
    ↓ synced via gRPC
ClientCache<LinkSys> on gateway
    ↓ watch events (add/update/delete)
LinkSysConfHandler (ConfHandler impl)
    ↓ creates/updates/destroys runtime clients
LinkSysRuntimeStore (ArcSwap<HashMap<String, Arc<dyn LinkSysClient>>>)
    ↓ get_redis_client("namespace/name")
Concrete client (e.g., RedisLinkClient)
    ↓ used by
Plugins, DataSender impls, etc.
```

**No separate store needed in user code.** The `ConfHandler` pattern (same as `PluginStore` for `EdgionPlugins`) ties the runtime client lifecycle directly to the CRD lifecycle. When a LinkSys CRD is created, the handler builds the client; when updated, it swaps; when deleted, it shuts down. Callers just call `get_redis_client("ns/name")`.

## Directory Structure

```
src/
├── core/link_sys/
│   ├── mod.rs                          # Re-exports: DataSender, LocalFileWriter, LogType, + new systems
│   ├── data_sender_trait.rs            # DataSender<T> trait (async init/send/healthy/name)
│   ├── local_file/                     # LocalFileWriter implementation (file + rotation)
│   │   ├── mod.rs
│   │   ├── data_sender_impl.rs
│   │   └── rotation.rs
│   ├── <system>/                       # New system connector (e.g., redis/)
│   │   ├── mod.rs                      # pub use client::...; pub use ops::...;
│   │   ├── client.rs                   # Client struct: init, connect, health check, shutdown
│   │   └── ops.rs                      # High-level operations (e.g., get/set/lock for Redis)
│   └── runtime_store.rs                # ConfHandler + ArcSwap store (driven by ClientCache watch)
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

1. **Runtime client** — `src/core/link_sys/<system>/client.rs`
2. **Operations** — `src/core/link_sys/<system>/ops.rs`
3. **Module exports** — `src/core/link_sys/<system>/mod.rs` + `src/core/link_sys/mod.rs`
4. **ConfHandler + runtime store** — Register in `runtime_store.rs` (follows PluginStore pattern)
5. **Cargo.toml** — Add the library dependency with appropriate features

### If CRD Config Types Don't Exist

Also add:

6. **CRD config** — `src/types/resources/link_sys/<system>.rs`
7. **SystemConfig variant** — Add to `SystemConfig` enum in `link_sys/mod.rs`
8. **Validation** — Add to `LinkSys::validate_config()`
9. **CRD YAML** — Update `config/crd/edgion-crd/link_sys_crd.yaml`

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
    "i-scripts",          # Lua scripting (for distributed locks)
] }
```

**Feature selection rules:**
- Only enable features you actually need (minimize compile time)
- Prefer `rustls` over `native-tls` for TLS (consistent with Edgion project)
- Enable `partial-tracing` or `metrics` only if observability integration is planned

### Step 2: Create Client Module

`src/core/link_sys/<system>/client.rs`:

```rust
use anyhow::Result;

/// Runtime client wrapper for <System>.
/// Built from LinkSys CRD config, managed by LinkSysRuntimeStore (ConfHandler-driven).
pub struct <System>LinkClient {
    // Library-native client type
    inner: <library>::Client,
    // Metadata
    name: String,
    healthy: std::sync::atomic::AtomicBool,
}

impl <System>LinkClient {
    /// Create a new client from CRD config.
    /// Does NOT connect yet — call init() to establish connection.
    pub fn from_config(name: &str, config: &<System>ClientConfig) -> Result<Self> {
        // Map CRD config → library config
        // ...
        Ok(Self { inner, name: name.to_string(), healthy: AtomicBool::new(false) })
    }

    /// Initialize connection. Called once after construction.
    pub async fn init(&self) -> Result<()> {
        // Connect, authenticate, verify
        // Set healthy = true on success
    }

    /// Check if the client is connected and responsive.
    pub fn healthy(&self) -> bool {
        self.healthy.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Graceful shutdown.
    pub async fn shutdown(&self) -> Result<()> {
        // Close connections, release resources
    }

    /// Get the underlying library client for advanced operations.
    /// Prefer using ops.rs high-level functions when possible.
    pub fn inner(&self) -> &<library>::Client {
        &self.inner
    }
}
```

**Key principles:**
- `from_config()` maps CRD types → library config (no library types in CRD layer)
- `init()` is separate from construction (allows lazy or deferred connection)
- `healthy()` is atomic, cheap, and safe for concurrent reads
- Expose `inner()` for advanced use cases that ops.rs doesn't cover

### Step 3: Create Operations Module

`src/core/link_sys/<system>/ops.rs`:

Define high-level operations that plugins and other code will use. Don't wrap every library function — only expose what Edgion actually needs:

```rust
impl <System>LinkClient {
    /// Example: Get a value by key
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        // Library-specific call
    }

    /// Example: Set a value with optional TTL
    pub async fn set(&self, key: &str, value: &str, ttl: Option<Duration>) -> Result<()> {
        // Library-specific call
    }
}
```

**Design tips:**
- Group related operations logically (CRUD, locking, pub/sub)
- Use standard Rust types in signatures (`String`, `Duration`, `Option<T>`)
- Let the library handle serialization/deserialization internally
- Return `anyhow::Result` for consistency with `DataSender`
- Add doc comments explaining the Redis/Etcd/etc. command being called

### Step 4: Export and Register

`src/core/link_sys/<system>/mod.rs`:

```rust
mod client;
pub mod ops;  // or include ops in client.rs if small

pub use client::<System>LinkClient;
```

`src/core/link_sys/mod.rs` — add:

```rust
pub mod <system>;
pub use <system>::<System>LinkClient;
```

### Step 5: ConfHandler + Runtime Store

LinkSys runtime clients are managed via the **ConfHandler pattern** (same as `PluginStore` for `EdgionPlugins`). The `ClientCache<LinkSys>` watch events drive the lifecycle automatically — no separate store management needed by callers.

```rust
// runtime_store.rs — follows the PluginStore pattern

use std::sync::Arc;
use arc_swap::ArcSwap;
use std::collections::HashMap;

/// Runtime store for LinkSys clients, driven by ClientCache<LinkSys> events.
/// Uses ArcSwap for lock-free reads (same pattern as PluginStore).
static LINK_SYS_RUNTIME: LazyLock<ArcSwap<HashMap<String, Arc<dyn LinkSysClient>>>> =
    LazyLock::new(|| ArcSwap::from_pointee(HashMap::new()));

/// ConfHandler implementation — receives full_set / partial_update from ClientCache.
/// On add/update: build runtime client from CRD config, init, store.
/// On delete: shutdown old client, remove from store.
impl ConfHandler for LinkSysRuntimeHandler {
    fn full_set(&self, data: HashMap<String, LinkSys>) {
        // For each LinkSys: match SystemConfig → build client → init → insert
        // Atomically swap the entire store
    }

    fn partial_update(&self, key: &str, change: Change, resource: Option<&LinkSys>) {
        // Add/Update: build new client, swap in store, shutdown old in background
        // Delete: remove from store, shutdown in background
    }
}

// Callers just use these functions — lifecycle is automatic:
pub fn get_redis_client(name: &str) -> Option<Arc<RedisLinkClient>> { /* ... */ }
pub fn get_link_sys_client(name: &str) -> Option<Arc<dyn LinkSysClient>> { /* ... */ }
```

**Key insight:** Callers never manage client lifecycle. The `ConfHandler` reacts to CRD changes from the existing `ClientCache<LinkSys>` watch stream — when a CRD is created, the client is built and connected; when updated, it's swapped; when deleted, it's shut down. This is the same pattern as how `PluginStore` manages plugin instances.

## CRD Config ↔ Runtime Client Mapping

The CRD config types (`types/resources/link_sys/<system>.rs`) define what users configure in YAML. The runtime client (`core/link_sys/<system>/client.rs`) translates this into library-specific config.

```
User YAML → CRD Type (serde) → from_config() → Library Config → Library Client
```

**Important:** CRD types must be serializable (serde + JsonSchema). They should NOT contain library-specific types. The `from_config()` method bridges the two worlds.

Example mapping (Redis):

| CRD Config | Library Config |
|------------|---------------|
| `endpoints: ["redis://..."]` | `fred::Config::from_url()` or `ServerConfig` |
| `topology.mode: cluster` | `fred::Config { server: ServerConfig::Cluster { ... } }` |
| `pool.size: 10` | `fred::Builder::set_pool_size(10)` |
| `auth.password` | `fred::Config { password: Some(...) }` |
| `tls.enabled: true` | `fred::Builder::with_tls_config(...)` |
| `timeout.connect: 5000` | `fred::Builder::with_connection_config(|c| c.connection_timeout = ...)` |

## Health Check Pattern

Every LinkSys client should support health checking for observability:

```rust
/// Health status of a LinkSys client
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
- **Surfaced via admin API** — `/api/v1/link-sys/health` or similar

## DataSender Integration

If the system is used as a log sink (ES, Kafka, Redis as failed cache), implement `DataSender<T>`:

```rust
#[async_trait]
impl DataSender<String> for <System>LinkClient {
    async fn init(&mut self) -> Result<()> { self.init().await }
    fn healthy(&self) -> bool { self.healthy() }
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
use crate::core::conf_mgr::sync_runtime::resource_processor::get_secret;

if let Some(secret_ref) = &config.auth.secret_ref {
    let secret = get_secret(Some(namespace), &secret_ref.name);
    // Extract password/cert from secret data
}
```

At runtime (gateway side), secrets are already resolved into `resolved_*` fields.

## Testing

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_config_mapping() {
        // Verify CRD config → library config mapping
        let crd_config = <System>ClientConfig { ... };
        let client = <System>LinkClient::from_config("test", &crd_config);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_health_check_when_disconnected() {
        // Client created but not init'd should report unhealthy
    }
}
```

### Integration Tests — Independent Test Scripts

Each LinkSys system has its own **independent integration test script** in `examples/test/scripts/integration/`, following the same pattern as `run_acme_test.sh`:

```
examples/test/scripts/integration/
├── run_acme_test.sh              # ACME tests (existing)
├── run_redis_test.sh             # Redis LinkSys integration tests
├── run_etcd_test.sh              # Etcd LinkSys integration tests (future)
└── ...
```

**Script structure** (following `run_acme_test.sh` pattern):

```bash
#!/bin/bash
# 1. Start external system (Docker Compose: Redis standalone/cluster/sentinel)
# 2. Wait for system to be ready (health check loop)
# 3. Build test client
# 4. Run integration tests (cargo run --example test_client -- -r LinkSys -i redis)
# 5. Cleanup (trap EXIT → docker compose down)
```

Each script:
- **Self-contained**: starts its own dependencies via Docker Compose, cleans up on exit
- **Supports `--no-cleanup`**: keep containers running for debugging
- **Supports test filter**: `./run_redis_test.sh standalone` to run only standalone tests
- **Logs colored output**: `[INFO]`, `[✓]`, `[✗]` for easy reading

**Test scenarios** (per system):
- Connection (standalone / cluster / sentinel)
- Authentication (password, ACL, wrong password)
- TLS (encrypted connection)
- Operations (GET/SET, HGET/HSET, distributed lock, etc.)
- Pool (concurrent requests under load)
- Auto-reconnect (disconnect → reconnect → verify recovery)
- Health check (healthy → unhealthy → healthy transitions)
- ConfHandler lifecycle (CRD create → client created, CRD update → client swapped, CRD delete → client shutdown)

## Coding Principles

1. **Comments in English** — all code comments, doc comments, and log messages
2. **Library-first design** — don't fight the library's API; wrap minimally
3. **Fail loudly on init, recover silently at runtime** — startup failures should be clear; runtime disconnects should auto-reconnect
4. **No secrets in logs** — use `tracing` for operational logs; never log passwords, tokens, or connection strings with credentials
5. **Feature-gate heavy dependencies** — if the library is large (e.g., rdkafka), consider a Cargo feature to make it optional