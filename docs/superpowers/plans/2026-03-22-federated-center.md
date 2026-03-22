# Federated Center Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `edgion-center` binary and the controller-side client that connects to it, enabling a federated center to periodically aggregate resource keys from multiple controllers and dispatch commands back.

**Architecture:** Controller (gRPC client) connects to center (gRPC server) via a persistent bidirectional stream. Controller sends: Register on connect, Pong for heartbeats, ListResponse when polled. Center sends: RegisterAck, Ping for heartbeat, ListRequest every 5 min, CommandRequest on demand. Center maintains an in-memory resource key snapshot per controller, organized by cluster.

**Tech Stack:** Rust, tonic 0.12 (gRPC), tokio (async runtime), axum 0.8 (Admin HTTP API), serde/serde_yaml (YAML parsing), uuid 1.x (request ID generation), clap (CLI).

**Spec:** `docs/superpowers/specs/2026-03-22-federated-center-design.md`

---

## File Map

### New Files
| Path | Purpose |
|------|---------|
| `src/core/common/fed_sync/proto/fed_sync.proto` | Federation gRPC service definition |
| `src/core/common/fed_sync/proto.rs` | tonic-generated types wrapper |
| `src/core/common/fed_sync/mod.rs` | Module exports |
| `src/core/controller/fed_sync/resource_collector/mod.rs` | Read ConfCenter → Vec\<ResourceKey\>, filter no_fed_sync_kinds |
| `src/core/controller/fed_sync/fed_client/mod.rs` | gRPC client: connect/register/heartbeat/reconnect/respond |
| `src/core/controller/fed_sync/mod.rs` | Module exports |
| `src/core/center/cli/mod.rs` | EdgionCenterCli + startup |
| `src/core/center/config/mod.rs` | CenterConfig (server + sync sections) |
| `src/core/center/fed_sync/server/mod.rs` | FederationGrpcServer (Connect RPC impl) |
| `src/core/center/fed_sync/registry/mod.rs` | ControllerRegistry (session management, pending map) |
| `src/core/center/fed_sync/mod.rs` | Module exports |
| `src/core/center/aggregator/mod.rs` | In-memory resource key snapshots per controller |
| `src/core/center/scheduler/mod.rs` | 5-min interval → send ListRequest to all online controllers |
| `src/core/center/commander/mod.rs` | Send CommandRequest, await response (30s timeout) |
| `src/core/center/api/mod.rs` | Admin HTTP API (query + command dispatch) |
| `src/core/center/mod.rs` | Module exports |
| `src/bin/edgion_center.rs` | Binary entry point |

### Modified Files
| Path | Change |
|------|--------|
| `build.rs` | Add fed_sync.proto compilation |
| `src/core/common/mod.rs` | Add `pub mod fed_sync` |
| `src/core/common/config/mod.rs` | Add `CenterClientConfig`, add `center` field to `EdgionControllerConfig` |
| `src/core/controller/mod.rs` | Add `pub mod fed_sync` |
| `src/core/controller/cli/mod.rs` | Spawn FederationClient if `center` config present |
| `src/core/mod.rs` | Add `pub mod center` |
| `src/lib.rs` | Export `EdgionCenterCli` |
| `Cargo.toml` | Add `[[bin]]` for edgion-center |

---

## Task 1: Proto Definition + Build Integration

**Files:**
- Create: `src/core/common/fed_sync/proto/fed_sync.proto`
- Create: `src/core/common/fed_sync/proto.rs`
- Create: `src/core/common/fed_sync/mod.rs`
- Modify: `src/core/common/mod.rs`
- Modify: `build.rs`

- [ ] **Step 1: Create the proto file**

```
src/core/common/fed_sync/proto/fed_sync.proto
```

Content — copy verbatim from spec Section 4 (the full proto definition). No changes needed.

- [ ] **Step 2: Create the proto wrapper module**

`src/core/common/fed_sync/proto.rs`:
```rust
mod inner {
    tonic::include_proto!("fed_sync");
}

pub use inner::*;

// Canonical Default impl for proto-generated RegisterRequest.
// Defined here (not in other modules) to avoid duplicate impl errors.
impl Default for RegisterRequest {
    fn default() -> Self {
        RegisterRequest {
            controller_id: String::new(),
            cluster: String::new(),
            env: vec![],
            tag: vec![],
            supported_kinds: vec![],
        }
    }
}
```

- [ ] **Step 3: Create the fed_sync module**

`src/core/common/fed_sync/mod.rs`:
```rust
pub mod proto;
```

- [ ] **Step 4: Register in common**

`src/core/common/mod.rs` — add line:
```rust
pub mod fed_sync;
```

- [ ] **Step 5: Update build.rs to compile fed_sync.proto**

In `build.rs`, after the existing `tonic_build::configure()...compile_protos(...)` call, add:

```rust
let fed_proto_dir = "src/core/common/fed_sync/proto";
tonic_build::configure()
    .file_descriptor_set_path(out_dir.join("fed_sync_descriptor.bin"))
    .compile_protos(&[format!("{}/fed_sync.proto", fed_proto_dir)], &[fed_proto_dir])?;
```

- [ ] **Step 6: Verify it compiles**

```bash
cargo check --all-targets 2>&1 | grep -E "error|warning.*fed_sync" | head -20
```

Expected: no errors related to fed_sync. Warnings about unused imports are OK at this stage.

- [ ] **Step 7: Commit**

```bash
git add src/core/common/fed_sync/ src/core/common/mod.rs build.rs
git commit -m "feat: add fed_sync proto and build integration"
```

---

## Task 2: Controller-Side Center Config

**Files:**
- Modify: `src/core/common/config/mod.rs`

- [ ] **Step 1: Write the failing test**

At the bottom of `src/core/common/config/mod.rs` tests section, add:

```rust
#[test]
fn test_center_client_config_parses_from_toml() {
    let toml = r#"
        [center]
        address = "http://center.example.com:50052"
        name = "ctrl-01"
        cluster = "prod-cn"
        env = ["production"]
        tag = ["team-infra"]
    "#;
    let config: EdgionControllerConfig = toml::from_str(toml).unwrap();
    let center = config.center.unwrap();
    assert_eq!(center.address, "http://center.example.com:50052");
    assert_eq!(center.name, "ctrl-01");
    assert_eq!(center.cluster, "prod-cn");
    assert_eq!(center.env, vec!["production"]);
    assert_eq!(center.ping_interval_secs, 30); // default
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test test_center_client_config_parses_from_toml 2>&1 | tail -5
```

Expected: compile error — `center` field not found.

- [ ] **Step 3: Add CenterClientConfig and field to EdgionControllerConfig**

In `src/core/common/config/mod.rs`, add this struct (near other config structs):

```rust
/// Configuration for connecting this controller to a federated center.
/// When absent, the federation client is not started.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CenterClientConfig {
    /// gRPC address of the federated center (e.g. "http://center.example.com:50052")
    pub address: String,
    /// Stable controller name; combined with `cluster` to form a stable controller_id across restarts
    pub name: String,
    /// Cluster this controller belongs to (used for center-side grouping)
    pub cluster: String,
    /// Environment tags (e.g. ["production"])
    pub env: Vec<String>,
    /// Arbitrary extension tags
    pub tag: Vec<String>,
    /// Heartbeat interval in seconds (default: 30)
    #[serde(default = "default_ping_interval_secs")]
    pub ping_interval_secs: u64,
}

fn default_ping_interval_secs() -> u64 { 30 }

impl CenterClientConfig {
    /// Build the stable controller_id from cluster + name
    pub fn controller_id(&self) -> String {
        format!("{}/{}", self.cluster, self.name)
    }
}
```

Then add to `EdgionControllerConfig`:
```rust
/// Optional federated center connection. Absent = feature disabled.
#[arg(skip)]
#[serde(default)]
pub center: Option<CenterClientConfig>,
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test test_center_client_config_parses_from_toml 2>&1 | tail -5
```

Expected: `test ... ok`

- [ ] **Step 5: Commit**

```bash
git add src/core/common/config/mod.rs
git commit -m "feat: add CenterClientConfig to controller config"
```

---

## Task 3: Resource Collector

**Files:**
- Create: `src/core/controller/fed_sync/resource_collector/mod.rs`
- Create: `src/core/controller/fed_sync/mod.rs`
- Modify: `src/core/controller/mod.rs` (add `pub mod fed_sync`)

**Context:** `ConfEntry` (from `conf_center/traits.rs`) has `kind`, `namespace`, `name`, `content` (raw YAML string). `CenterApi::get_list_by_kind()` returns all entries for a kind. We parse YAML to extract metadata fields for `ResourceKey`.

**Fed no_sync_kinds** (hardcoded in this module):
`["Secret", "ConfigMap", "Endpoint", "EndpointSlice", "ReferenceGrant"]`

- [ ] **Step 1: Write the failing test**

Create `src/core/controller/fed_sync/resource_collector/mod.rs` with only the test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_yaml(labels: &str, annotations: &str, resource_version: &str) -> String {
        format!(
            r#"apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
  resourceVersion: "{resource_version}"
  labels: {{{labels}}}
  annotations: {{{annotations}}}
spec: {{}}
"#
        )
    }

    #[test]
    fn test_extract_metadata_from_yaml() {
        let yaml = make_yaml(r#""app": "foo""#, r#""note": "bar""#, "12345");
        let key = extract_resource_key("HTTPRoute", &yaml).unwrap();
        assert_eq!(key.kind, "HTTPRoute");
        assert_eq!(key.resource_version, "12345");
        assert_eq!(key.labels.get("app").map(|s| s.as_str()), Some("foo"));
        assert_eq!(key.annotations.get("note").map(|s| s.as_str()), Some("bar"));
    }

    #[test]
    fn test_fed_no_sync_kinds_are_excluded() {
        for kind in FED_NO_SYNC_KINDS {
            assert!(is_fed_no_sync_kind(kind));
        }
        assert!(!is_fed_no_sync_kind("HTTPRoute"));
        assert!(!is_fed_no_sync_kind("Gateway"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p edgion fed_sync::resource_collector 2>&1 | tail -5
```

Expected: compile error.

- [ ] **Step 3: Implement resource_collector**

Full content of `src/core/controller/fed_sync/resource_collector/mod.rs`:

```rust
//! Reads raw resources from ConfCenter and converts them to ResourceKey (metadata only).
//! Filters out FED_NO_SYNC_KINDS on the controller side before reporting to center.

use crate::core::common::fed_sync::proto::{ResourceKey};
use crate::core::controller::conf_mgr::conf_center::traits::CenterApi;
use anyhow::Result;
use std::collections::HashMap;

/// Resource kinds excluded from federation sync (controller side filter).
pub const FED_NO_SYNC_KINDS: &[&str] = &[
    "Secret",
    "ConfigMap",
    "Endpoint",
    "EndpointSlice",
    "ReferenceGrant",
];

pub fn is_fed_no_sync_kind(kind: &str) -> bool {
    FED_NO_SYNC_KINDS.iter().any(|k| *k == kind)
}

/// Extract a ResourceKey from raw YAML content.
/// Returns None if YAML is unparseable (logs a warning, does not fail the whole list).
pub fn extract_resource_key(kind: &str, yaml_content: &str) -> Option<ResourceKey> {
    let value: serde_yaml::Value = serde_yaml::from_str(yaml_content).ok()?;
    let metadata = value.get("metadata")?;

    let name = metadata.get("name")?.as_str()?.to_string();
    let namespace = metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let resource_version = metadata
        .get("resourceVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let labels = extract_string_map(metadata.get("labels"));
    let annotations = extract_string_map(metadata.get("annotations"));

    Some(ResourceKey {
        kind: kind.to_string(),
        namespace,
        name,
        resource_version,
        labels,
        annotations,
    })
}

fn extract_string_map(value: Option<&serde_yaml::Value>) -> HashMap<String, String> {
    value
        .and_then(|v| v.as_mapping())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    let key = k.as_str()?.to_string();
                    let val = v.as_str()?.to_string();
                    Some((key, val))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Collect ResourceKeys for all non-excluded kinds from the ConfCenter.
///
/// `supported_kinds` is the full list of kinds available in this controller.
/// This function filters out FED_NO_SYNC_KINDS and fetches + parses each remaining kind.
pub async fn collect_resource_keys(
    conf_center: &dyn CenterApi,
    supported_kinds: &[String],
) -> Result<Vec<ResourceKey>> {
    let mut result = Vec::new();

    for kind in supported_kinds {
        if is_fed_no_sync_kind(kind) {
            continue;
        }

        let list_result = conf_center
            .get_list_by_kind(kind, None)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list {}: {}", kind, e))?;

        for entry in list_result.items {
            match extract_resource_key(&entry.kind, &entry.content) {
                Some(key) => result.push(key),
                None => {
                    tracing::warn!(
                        component = "resource_collector",
                        kind = %entry.kind,
                        name = %entry.name,
                        "Failed to parse resource metadata, skipping"
                    );
                }
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_yaml(labels: &str, annotations: &str, resource_version: &str) -> String {
        format!(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata:\n  name: my-route\n  namespace: default\n  resourceVersion: \"{resource_version}\"\n  labels:\n    {labels}\n  annotations:\n    {annotations}\nspec: {{}}\n"
        )
    }

    #[test]
    fn test_extract_metadata_from_yaml() {
        let yaml = make_yaml("app: foo", "note: bar", "12345");
        let key = extract_resource_key("HTTPRoute", &yaml).unwrap();
        assert_eq!(key.kind, "HTTPRoute");
        assert_eq!(key.resource_version, "12345");
        assert_eq!(key.labels.get("app").map(|s| s.as_str()), Some("foo"));
        assert_eq!(key.annotations.get("note").map(|s| s.as_str()), Some("bar"));
    }

    #[test]
    fn test_extract_missing_fields_defaults() {
        let yaml = "metadata:\n  name: bare\nspec: {}\n";
        let key = extract_resource_key("Gateway", yaml).unwrap();
        assert_eq!(key.name, "bare");
        assert_eq!(key.namespace, "");
        assert_eq!(key.resource_version, "");
        assert!(key.labels.is_empty());
    }

    #[test]
    fn test_fed_no_sync_kinds_are_excluded() {
        for kind in FED_NO_SYNC_KINDS {
            assert!(is_fed_no_sync_kind(kind));
        }
        assert!(!is_fed_no_sync_kind("HTTPRoute"));
        assert!(!is_fed_no_sync_kind("Gateway"));
    }
}
```

- [ ] **Step 4: Create the fed_sync mod files**

`src/core/controller/fed_sync/mod.rs`:
```rust
pub mod resource_collector;
```

- [ ] **Step 5: Register in controller**

In `src/core/controller/mod.rs`, add:
```rust
pub mod fed_sync;
```

(Check the existing file first to find the right place to add it.)

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test resource_collector 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/core/controller/fed_sync/ src/core/controller/mod.rs
git commit -m "feat: add resource_collector for federation sync"
```

---

## Task 4: Federation Client (Controller Side)

**Files:**
- Create: `src/core/controller/fed_sync/fed_client/mod.rs`
- Modify: `src/core/controller/fed_sync/mod.rs`

**What this does:** Connects to center's gRPC endpoint, sends `RegisterRequest` as first message, then loops handling:
- `CenterMessage::Ping` → send `Pong`
- `CenterMessage::ListRequest` → call `resource_collector`, send `ListResponse`
- `CenterMessage::CommandRequest` → execute command (apply/delete/reload via ConfCenter), send `CommandResponse`
- `CenterMessage::RegisterAck` → log session_id, continue

On stream error: exponential backoff reconnect (1s → 2s → 4s → ... → 60s cap).

- [ ] **Step 1: Write the failing tests**

Create `src/core/controller/fed_sync/fed_client/mod.rs` with only the test module initially:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_caps_at_60s() {
        let mut backoff = ReconnectBackoff::new();
        for _ in 0..20 {
            backoff.next();
        }
        assert_eq!(backoff.current_secs(), 60);
    }

    #[test]
    fn test_backoff_resets() {
        let mut backoff = ReconnectBackoff::new();
        backoff.next(); // 1s
        backoff.next(); // 2s
        backoff.reset();
        assert_eq!(backoff.current_secs(), 1);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test fed_client 2>&1 | tail -5
```

Expected: compile error.

- [ ] **Step 3: Implement the FederationClient**

Full content of `src/core/controller/fed_sync/fed_client/mod.rs`:

```rust
//! Federation client: connects to edgion-center and maintains a persistent gRPC stream.
//!
//! Lifecycle:
//! 1. Connect to center address
//! 2. Open bidirectional stream (Connect RPC)
//! 3. Send RegisterRequest as first message
//! 4. Loop: handle CenterMessage (Ping, ListRequest, CommandRequest, RegisterAck)
//! 5. On error: exponential backoff, reconnect

use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use tokio::sync::watch;
use tonic::transport::Channel;
use uuid::Uuid;

use crate::core::common::config::CenterClientConfig;
use crate::core::common::fed_sync::proto::{
    federation_sync_client::FederationSyncClient,
    center_message::Payload as CenterPayload,
    controller_message::Payload as CtrlPayload,
    CenterMessage, ControllerMessage,
    Pong, RegisterRequest, ListResponse, CommandResponse,
};
use crate::core::controller::conf_mgr::conf_center::traits::CenterApi;
use crate::core::controller::fed_sync::resource_collector;

/// Exponential backoff state for reconnection attempts.
pub struct ReconnectBackoff {
    current: u64,
}

impl ReconnectBackoff {
    pub fn new() -> Self { Self { current: 1 } }

    pub fn current_secs(&self) -> u64 { self.current }

    /// Advance to next backoff value (doubles, capped at 60s).
    pub fn next(&mut self) {
        self.current = (self.current * 2).min(60);
    }

    pub fn reset(&mut self) { self.current = 1; }
}

/// Runs the federation client loop. Should be spawned as a background task.
/// Exits only when `shutdown` is triggered.
pub async fn run(
    config: CenterClientConfig,
    conf_center: Arc<dyn CenterApi>,
    supported_kinds: Vec<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut backoff = ReconnectBackoff::new();

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                tracing::info!(component = "fed_client", "Shutdown signal received, exiting");
                return;
            }
            result = connect_and_run(&config, conf_center.clone(), &supported_kinds) => {
                match result {
                    Ok(()) => {
                        tracing::info!(component = "fed_client", "Stream ended cleanly, reconnecting");
                    }
                    Err(e) => {
                        tracing::warn!(
                            component = "fed_client",
                            error = %e,
                            retry_secs = backoff.current_secs(),
                            "Connection failed, will retry"
                        );
                    }
                }
            }
        }

        let wait = Duration::from_secs(backoff.current_secs());
        backoff.next();

        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => {
                tracing::info!(component = "fed_client", "Shutdown during backoff, exiting");
                return;
            }
        }
    }
}

async fn connect_and_run(
    config: &CenterClientConfig,
    conf_center: Arc<dyn CenterApi>,
    supported_kinds: &[String],
) -> Result<()> {
    let channel = Channel::from_shared(config.address.clone())?
        .connect()
        .await?;

    let mut client = FederationSyncClient::new(channel);

    let (tx, rx) = tokio::sync::mpsc::channel::<ControllerMessage>(32);
    let tx = Arc::new(tx);

    // Send RegisterRequest as first message
    let register = ControllerMessage {
        payload: Some(CtrlPayload::Register(RegisterRequest {
            controller_id: config.controller_id(),
            cluster: config.cluster.clone(),
            env: config.env.clone(),
            tag: config.tag.clone(),
            supported_kinds: supported_kinds
                .iter()
                .filter(|k| !resource_collector::is_fed_no_sync_kind(k))
                .cloned()
                .collect(),
        })),
    };
    tx.send(register).await?;

    let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut inbound = client.connect(outbound).await?.into_inner();

    tracing::info!(
        component = "fed_client",
        controller_id = %config.controller_id(),
        "Connected and registered with center"
    );

    let ping_interval = Duration::from_secs(config.ping_interval_secs);
    let mut ping_timer = tokio::time::interval(ping_interval);
    ping_timer.tick().await; // skip immediate first tick

    loop {
        tokio::select! {
            msg = inbound.message() => {
                match msg? {
                    None => {
                        tracing::info!(component = "fed_client", "Center closed stream");
                        return Ok(());
                    }
                    Some(CenterMessage { payload: Some(payload) }) => {
                        handle_center_message(
                            payload,
                            tx.clone(),
                            conf_center.clone(),
                            supported_kinds,
                        ).await?;
                    }
                    Some(_) => {} // empty payload, ignore
                }
            }
            _ = ping_timer.tick() => {
                // Note: ping is server-initiated; this timer is just a keepalive guard.
                // If center doesn't send Ping within interval*3, the stream will be
                // closed by center. Controller doesn't need to self-ping.
            }
        }
    }
}

async fn handle_center_message(
    payload: CenterPayload,
    tx: Arc<tokio::sync::mpsc::Sender<ControllerMessage>>,
    conf_center: Arc<dyn CenterApi>,
    supported_kinds: &[String],
) -> Result<()> {
    match payload {
        CenterPayload::RegisterAck(ack) => {
            tracing::info!(
                component = "fed_client",
                session_id = %ack.session_id,
                "Registered with center"
            );
        }

        CenterPayload::Ping(ping) => {
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::Pong(Pong {
                    timestamp: ping.timestamp,
                })),
            }).await?;
        }

        CenterPayload::ListRequest(req) => {
            tracing::debug!(
                component = "fed_client",
                request_id = %req.request_id,
                "ListRequest received"
            );
            let keys = match resource_collector::collect_resource_keys(
                conf_center.as_ref(),
                supported_kinds,
            ).await {
                Ok(keys) => keys,
                Err(e) => {
                    tracing::error!(component = "fed_client", error = %e, "collect_resource_keys failed");
                    vec![]
                }
            };
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::ListResponse(ListResponse {
                    request_id: req.request_id,
                    keys,
                })),
            }).await?;
        }

        CenterPayload::Command(cmd) => {
            tracing::info!(
                component = "fed_client",
                request_id = %cmd.request_id,
                "CommandRequest received"
            );
            // Command execution (apply/delete/reload) — stubbed for now
            // Full implementation requires access to ConfMgr which is wired in Task 5
            let _ = conf_center; // suppress unused warning
            tx.send(ControllerMessage {
                payload: Some(CtrlPayload::CommandResponse(CommandResponse {
                    request_id: cmd.request_id,
                    success: false,
                    message: "Command execution not yet implemented".to_string(),
                })),
            }).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_caps_at_60s() {
        let mut backoff = ReconnectBackoff::new();
        for _ in 0..20 {
            backoff.next();
        }
        assert_eq!(backoff.current_secs(), 60);
    }

    #[test]
    fn test_backoff_resets() {
        let mut backoff = ReconnectBackoff::new();
        backoff.next(); // 2
        backoff.next(); // 4
        backoff.reset();
        assert_eq!(backoff.current_secs(), 1);
    }
}
```

- [ ] **Step 4: Update fed_sync mod**

`src/core/controller/fed_sync/mod.rs`:
```rust
pub mod fed_client;
pub mod resource_collector;
```

- [ ] **Step 5: Run tests**

```bash
cargo test fed_client 2>&1 | tail -10
```

Expected: backoff tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/core/controller/fed_sync/
git commit -m "feat: add FederationClient with exponential backoff reconnect"
```

---

## Task 5: Controller Startup Integration

**Files:**
- Modify: `src/core/controller/cli/mod.rs`

**Goal:** If `config.center.is_some()`, spawn `fed_client::run` as a background tokio task after ConfMgr is ready. Pass a reference to ConfCenter and the list of supported_kinds from ProcessorRegistry.

- [ ] **Step 1: Read the current cli/mod.rs startup sequence**

Run: `cat -n src/core/controller/cli/mod.rs | head -120`

Identify where `conf_mgr.start(shutdown_handle)` is called. The fed_client spawn goes just before that line (ConfMgr must be initialized first to provide supported_kinds).

- [ ] **Step 2: Add federation client startup**

In `src/core/controller/cli/mod.rs`, **after `conf_mgr.start(shutdown_handle).await` completes** (i.e., after initial sync is done and processors are registered), spawn the federation client. Look for where the controller enters its main run loop and add just before the final await:

```rust
// Start federation client if center config is present.
// Must be after conf_mgr.start() so PROCESSOR_REGISTRY is populated.
let _fed_shutdown_tx = if let Some(center_config) = config.center.clone() {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Arc<dyn CenterApi>: ConfCenter is a supertrait of CenterApi, so coerce directly
    let center_api: Arc<dyn crate::core::controller::conf_mgr::conf_center::traits::CenterApi> =
        conf_mgr.conf_center();  // conf_center() returns Arc<dyn ConfCenter> which is also Arc<dyn CenterApi>

    // Supported kinds come from the global PROCESSOR_REGISTRY (populated during start())
    let kinds: Vec<String> = crate::core::controller::conf_mgr::processor_registry::PROCESSOR_REGISTRY
        .all_kinds()
        .iter()
        .map(|s| s.to_string())
        .collect();

    tokio::spawn(
        crate::core::controller::fed_sync::fed_client::run(
            center_config,
            center_api,
            kinds,
            shutdown_rx,
        )
    );

    Some(shutdown_tx) // kept alive for duration of process
} else {
    None
};
```

> **Note:** `ConfCenter` is a supertrait of `CenterApi` (see `conf_center/traits.rs`). If the coercion doesn't compile directly, add a thin helper method `fn as_center_api(c: Arc<dyn ConfCenter>) -> Arc<dyn CenterApi>` or check whether `ConfCenter: CenterApi` allows the `Arc` cast. Adjust as needed based on the actual trait hierarchy.

- [ ] **Step 3: Verify it compiles**

```bash
cargo check --bin edgion-controller 2>&1 | grep "error" | head -10
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/core/controller/cli/mod.rs
git commit -m "feat: spawn FederationClient in controller startup if center config present"
```

---

## Task 6: Center Binary Skeleton + Config

**Files:**
- Create: `src/core/center/config/mod.rs`
- Create: `src/core/center/cli/mod.rs`
- Create: `src/core/center/mod.rs`
- Create: `src/bin/edgion_center.rs`
- Modify: `src/core/mod.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Write the failing config test**

Create `src/core/center/config/mod.rs` with only the test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_center_config_defaults() {
        let config = CenterConfig::default();
        assert_eq!(config.server.grpc_addr, "0.0.0.0:50052");
        assert_eq!(config.server.http_addr, "0.0.0.0:5810");
        assert_eq!(config.sync.list_interval_secs, 300);
        assert_eq!(config.sync.ping_interval_secs, 30);
        assert_eq!(config.sync.list_timeout_secs, 30);
        assert_eq!(config.sync.command_timeout_secs, 30);
        assert_eq!(config.sync.offline_evict_hours, 24);
    }

    #[test]
    fn test_center_config_parses_from_toml() {
        let toml = r#"
            [server]
            grpc_addr = "0.0.0.0:50100"
            http_addr = "0.0.0.0:5900"
            [sync]
            list_interval_secs = 60
        "#;
        let config: CenterConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.server.grpc_addr, "0.0.0.0:50100");
        assert_eq!(config.sync.list_interval_secs, 60);
        assert_eq!(config.sync.ping_interval_secs, 30); // default
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test center::config 2>&1 | tail -5
```

Expected: compile error.

- [ ] **Step 3: Implement CenterConfig**

Full `src/core/center/config/mod.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CenterConfig {
    pub server: CenterServerConfig,
    pub sync: CenterSyncConfig,
}

impl Default for CenterConfig {
    fn default() -> Self {
        Self {
            server: CenterServerConfig::default(),
            sync: CenterSyncConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CenterServerConfig {
    /// gRPC listen address for federation sync
    pub grpc_addr: String,
    /// HTTP listen address for admin API
    pub http_addr: String,
}

impl Default for CenterServerConfig {
    fn default() -> Self {
        Self {
            grpc_addr: "0.0.0.0:50052".to_string(),
            http_addr: "0.0.0.0:5810".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CenterSyncConfig {
    /// How often to send ListRequest to each controller (seconds, default: 300 = 5min)
    pub list_interval_secs: u64,
    /// Timeout waiting for ListResponse (seconds, default: 30)
    pub list_timeout_secs: u64,
    /// Timeout waiting for CommandResponse (seconds, default: 30)
    pub command_timeout_secs: u64,
    /// Heartbeat ping interval sent to controllers (seconds, default: 30)
    pub ping_interval_secs: u64,
    /// Hours before offline controller data is evicted (default: 24)
    pub offline_evict_hours: u64,
}

impl Default for CenterSyncConfig {
    fn default() -> Self {
        Self {
            list_interval_secs: 300,
            list_timeout_secs: 30,
            command_timeout_secs: 30,
            ping_interval_secs: 30,
            offline_evict_hours: 24,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // ... (paste test from step 1 here)
}
```

- [ ] **Step 4: Create module files**

`src/core/center/mod.rs`:
```rust
pub mod cli;
pub mod config;
pub mod aggregator;
pub mod commander;
pub mod fed_sync;
pub mod scheduler;
pub mod api;
```

`src/core/center/cli/mod.rs` (minimal skeleton):
```rust
use anyhow::Result;
use clap::Parser;
use crate::core::center::config::CenterConfig;

#[derive(Parser, Debug)]
#[command(name = "edgion-center", version, about = "Edgion Federated Center")]
pub struct EdgionCenterCli {
    /// Configuration file path (TOML)
    #[arg(short = 'c', long, default_value = "config/edgion-center.toml")]
    pub config_file: String,
}

impl EdgionCenterCli {
    pub fn parse_args() -> Self { Self::parse() }

    pub async fn run(&self) -> Result<()> {
        let content = std::fs::read_to_string(&self.config_file)
            .unwrap_or_default();
        let _config: CenterConfig = toml::from_str(&content).unwrap_or_default();
        tracing::info!(component = "center", "edgion-center starting (stub)");
        // Full startup wired in later tasks
        Ok(())
    }
}
```

- [ ] **Step 5: Create placeholder mods for modules referenced in center/mod.rs**

Create empty `mod.rs` files with `// TODO` for:
- `src/core/center/aggregator/mod.rs`
- `src/core/center/commander/mod.rs`
- `src/core/center/fed_sync/mod.rs`
- `src/core/center/scheduler/mod.rs`
- `src/core/center/api/mod.rs`

Each just:
```rust
// TODO: implement in subsequent tasks
```

- [ ] **Step 6: Wire the binary**

`src/bin/edgion_center.rs`:
```rust
use edgion::EdgionCenterCli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cli = EdgionCenterCli::parse_args();
    if let Err(err) = cli.run().await {
        eprintln!("Error: {:#}", err);
        std::process::exit(1);
    }
}
```

`src/core/mod.rs` — add:
```rust
pub mod center;
```

`src/lib.rs` — add:
```rust
pub use crate::core::center::cli::EdgionCenterCli;
```

`Cargo.toml` — add after existing `[[bin]]` entries:
```toml
[[bin]]
name = "edgion-center"
path = "src/bin/edgion_center.rs"
```

- [ ] **Step 7: Run tests and verify binary compiles**

```bash
cargo test center::config 2>&1 | tail -10
cargo check --bin edgion-center 2>&1 | grep "error" | head -10
```

Expected: config tests pass, binary compiles.

- [ ] **Step 8: Commit**

```bash
git add src/core/center/ src/bin/edgion_center.rs src/core/mod.rs src/lib.rs Cargo.toml
git commit -m "feat: add edgion-center binary skeleton and config"
```

---

## Task 7: Center gRPC Server + Registry

**Files:**
- Create: `src/core/center/fed_sync/registry/mod.rs`
- Create: `src/core/center/fed_sync/server/mod.rs`
- Modify: `src/core/center/fed_sync/mod.rs`

**Key design:**
- `ControllerRegistry` holds `HashMap<controller_id, ControllerSession>` behind `Arc<RwLock<...>>`
- `ControllerSession` holds: `info: RegisterRequest`, `stream_tx: mpsc::Sender<CenterMessage>`, `last_seen: Instant`, `offline_since: Option<Instant>`, `session_id: String`
- `FederationGrpcServer` implements the `FederationSync` service
- On `Connect()`: spawn task reading first message (5s timeout) → must be `RegisterRequest` → register → loop handling remaining messages
- Heartbeat: spawn background task per session sending `Ping` every `ping_interval`

- [ ] **Step 1: Write failing tests for registry**

Create `src/core/center/fed_sync/registry/mod.rs` with test-only stub:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_lookup() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        registry.register("cluster/ctrl-01".to_string(), mock_info(), tx, "sess-1".to_string());
        assert!(registry.get_session("cluster/ctrl-01").is_some());
        assert_eq!(registry.online_controller_ids().len(), 1);
    }

    #[test]
    fn test_mark_offline_and_evict() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        registry.register("cid".to_string(), mock_info(), tx, "s1".to_string());
        registry.mark_offline("cid");
        assert!(registry.get_session("cid").map(|s| s.offline_since.is_some()).unwrap_or(false));
        // evict entries older than 0 hours
        registry.evict_stale(0);
        assert!(registry.get_session("cid").is_none());
    }

    fn mock_info() -> crate::core::common::fed_sync::proto::RegisterRequest {
        crate::core::common::fed_sync::proto::RegisterRequest {
            controller_id: "cluster/ctrl-01".to_string(),
            cluster: "cluster".to_string(),
            env: vec![],
            tag: vec![],
            supported_kinds: vec![],
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test center::fed_sync::registry 2>&1 | tail -5
```

Expected: compile error.

- [ ] **Step 3: Implement ControllerRegistry**

Full `src/core/center/fed_sync/registry/mod.rs`:

```rust
//! ControllerRegistry: manages per-controller sessions and stream handles.
//!
//! controller_id (stable: "cluster/name") → ControllerSession

use crate::core::common::fed_sync::proto::{CenterMessage, RegisterRequest};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Debug)]
pub struct ControllerSession {
    pub controller_id: String,
    pub session_id: String,
    pub info: RegisterRequest,
    /// Sender for CenterMessage → this controller's stream
    pub stream_tx: mpsc::Sender<CenterMessage>,
    pub last_seen: Instant,
    pub offline_since: Option<Instant>,
}

impl ControllerSession {
    pub fn is_online(&self) -> bool {
        self.offline_since.is_none()
    }
}

#[derive(Clone)]
pub struct ControllerRegistry {
    inner: Arc<RwLock<HashMap<String, ControllerSession>>>,
}

impl ControllerRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register(
        &self,
        controller_id: String,
        info: RegisterRequest,
        stream_tx: mpsc::Sender<CenterMessage>,
        session_id: String,
    ) {
        let session = ControllerSession {
            controller_id: controller_id.clone(),
            session_id,
            info,
            stream_tx,
            last_seen: Instant::now(),
            offline_since: None,
        };
        let mut map = self.inner.write().unwrap();
        map.insert(controller_id, session);
    }

    pub fn get_session(&self, controller_id: &str) -> Option<SessionView> {
        let map = self.inner.read().unwrap();
        map.get(controller_id).map(|s| SessionView {
            controller_id: s.controller_id.clone(),
            session_id: s.session_id.clone(),
            info: s.info.clone(),
            stream_tx: s.stream_tx.clone(),
            last_seen: s.last_seen,
            offline_since: s.offline_since,
        })
    }

    pub fn update_last_seen(&self, controller_id: &str) {
        if let Some(session) = self.inner.write().unwrap().get_mut(controller_id) {
            session.last_seen = Instant::now();
        }
    }

    pub fn mark_offline(&self, controller_id: &str) {
        if let Some(session) = self.inner.write().unwrap().get_mut(controller_id) {
            if session.offline_since.is_none() {
                session.offline_since = Some(Instant::now());
                tracing::info!(
                    component = "registry",
                    controller_id = %controller_id,
                    "Controller marked offline"
                );
            }
        }
    }

    /// Returns controller_ids of currently online controllers.
    pub fn online_controller_ids(&self) -> Vec<String> {
        self.inner
            .read()
            .unwrap()
            .values()
            .filter(|s| s.is_online())
            .map(|s| s.controller_id.clone())
            .collect()
    }

    /// Returns stream senders for all online controllers.
    pub fn online_senders(&self) -> Vec<(String, mpsc::Sender<CenterMessage>)> {
        self.inner
            .read()
            .unwrap()
            .values()
            .filter(|s| s.is_online())
            .map(|s| (s.controller_id.clone(), s.stream_tx.clone()))
            .collect()
    }

    /// Evict sessions that have been offline for longer than `hours`.
    pub fn evict_stale(&self, hours: u64) {
        let threshold = Duration::from_secs(hours * 3600);
        self.inner.write().unwrap().retain(|id, session| {
            if let Some(since) = session.offline_since {
                if since.elapsed() > threshold {
                    tracing::info!(component = "registry", controller_id = %id, "Evicting stale offline controller");
                    return false;
                }
            }
            true
        });
    }
}

/// A read-only snapshot of session data (avoids holding the lock).
#[derive(Debug, Clone)]
pub struct SessionView {
    pub controller_id: String,
    pub session_id: String,
    pub info: RegisterRequest,
    pub stream_tx: mpsc::Sender<CenterMessage>,
    pub last_seen: Instant,
    pub offline_since: Option<Instant>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_info(cid: &str) -> RegisterRequest {
        RegisterRequest {
            controller_id: cid.to_string(),
            cluster: "cluster".to_string(),
            env: vec![],
            tag: vec![],
            supported_kinds: vec![],
        }
    }

    #[test]
    fn test_register_and_lookup() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        registry.register("cluster/ctrl-01".to_string(), mock_info("cluster/ctrl-01"), tx, "sess-1".to_string());
        assert!(registry.get_session("cluster/ctrl-01").is_some());
        assert_eq!(registry.online_controller_ids().len(), 1);
    }

    #[test]
    fn test_mark_offline_and_evict() {
        let registry = ControllerRegistry::new();
        let (tx, _rx) = mpsc::channel(8);
        registry.register("cid".to_string(), mock_info("cid"), tx, "s1".to_string());
        registry.mark_offline("cid");
        assert!(registry.get_session("cid").map(|s| s.offline_since.is_some()).unwrap_or(false));
        registry.evict_stale(0);
        assert!(registry.get_session("cid").is_none());
    }

    #[test]
    fn test_online_senders_excludes_offline() {
        let registry = ControllerRegistry::new();
        let (tx1, _rx1) = mpsc::channel(8);
        let (tx2, _rx2) = mpsc::channel(8);
        registry.register("c1".to_string(), mock_info("c1"), tx1, "s1".to_string());
        registry.register("c2".to_string(), mock_info("c2"), tx2, "s2".to_string());
        registry.mark_offline("c2");
        let senders = registry.online_senders();
        assert_eq!(senders.len(), 1);
        assert_eq!(senders[0].0, "c1");
    }
}
```

- [ ] **Step 4: Implement FederationGrpcServer**

Create `src/core/center/fed_sync/server/mod.rs`:

```rust
//! gRPC server implementing FederationSync::Connect.
//!
//! On each Connect() call:
//! 1. Wait up to 5s for first ControllerMessage (must be RegisterRequest)
//! 2. Register controller in registry
//! 3. Spawn heartbeat task (Ping every ping_interval)
//! 4. Loop: forward incoming messages to aggregator/commander; forward outgoing to stream

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tonic::{Request, Response, Status, Streaming};
use uuid::Uuid;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::config::CenterSyncConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::common::fed_sync::proto::{
    federation_sync_server::FederationSync,
    center_message::Payload as CenterPayload,
    controller_message::Payload as CtrlPayload,
    CenterMessage, ControllerMessage,
    Ping, RegisterAck,
};

/// Pending ListRequest entry: sender + timestamp for timeout tracking
pub struct PendingListEntry {
    pub sender: oneshot::Sender<Vec<crate::core::common::fed_sync::proto::ResourceKey>>,
    pub created_at: std::time::Instant,
}
/// Pending ListRequest map: request_id → entry (sender + timestamp)
pub type PendingListMap = Arc<Mutex<HashMap<String, PendingListEntry>>>;
/// Pending CommandRequest map: request_id → oneshot sender for CommandResponse
pub type PendingCommandMap = Arc<Mutex<HashMap<String, oneshot::Sender<crate::core::common::fed_sync::proto::CommandResponse>>>>;

pub struct FederationGrpcServer {
    pub registry: ControllerRegistry,
    pub aggregator: Arc<ResourceAggregator>,
    pub pending_lists: PendingListMap,
    pub pending_commands: PendingCommandMap,
    pub sync_config: CenterSyncConfig,
}

impl FederationGrpcServer {
    pub fn new(
        registry: ControllerRegistry,
        aggregator: Arc<ResourceAggregator>,
        sync_config: CenterSyncConfig,
    ) -> Self {
        Self {
            registry,
            aggregator,
            pending_lists: Arc::new(Mutex::new(HashMap::new())),
            pending_commands: Arc::new(Mutex::new(HashMap::new())),
            sync_config,
        }
    }

    pub fn into_service(self) -> crate::core::common::fed_sync::proto::federation_sync_server::FederationSyncServer<Self> {
        crate::core::common::fed_sync::proto::federation_sync_server::FederationSyncServer::new(self)
    }
}

#[tonic::async_trait]
impl FederationSync for FederationGrpcServer {
    type ConnectStream = tokio_stream::wrappers::ReceiverStream<Result<CenterMessage, Status>>;

    async fn connect(
        &self,
        request: Request<Streaming<ControllerMessage>>,
    ) -> Result<Response<Self::ConnectStream>, Status> {
        let mut inbound = request.into_inner();
        let (out_tx, out_rx) = mpsc::channel::<Result<CenterMessage, Status>>(32);
        let (inner_tx, mut inner_rx) = mpsc::channel::<CenterMessage>(32);

        // 1. Wait for RegisterRequest (5s timeout)
        let first_msg = tokio::time::timeout(
            Duration::from_secs(5),
            inbound.message(),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Registration timeout: no RegisterRequest within 5s"))?
        .map_err(|e| Status::internal(e.to_string()))?
        .ok_or_else(|| Status::cancelled("Stream closed before RegisterRequest"))?;

        let register_req = match first_msg.payload {
            Some(CtrlPayload::Register(r)) => r,
            _ => return Err(Status::invalid_argument("First message must be RegisterRequest")),
        };

        let controller_id = register_req.controller_id.clone();
        let session_id = Uuid::new_v4().to_string();

        tracing::info!(
            component = "fed_server",
            controller_id = %controller_id,
            session_id = %session_id,
            cluster = %register_req.cluster,
            "Controller registered"
        );

        // 2. Register in registry
        self.registry.register(
            controller_id.clone(),
            register_req,
            inner_tx.clone(),
            session_id.clone(),
        );

        // Send RegisterAck
        let _ = inner_tx.send(CenterMessage {
            payload: Some(CenterPayload::RegisterAck(RegisterAck { session_id })),
        }).await;

        let registry = self.registry.clone();
        let aggregator = self.aggregator.clone();
        let pending_lists = self.pending_lists.clone();
        let pending_commands = self.pending_commands.clone();
        let ping_interval = Duration::from_secs(self.sync_config.ping_interval_secs);
        let heartbeat_timeout = ping_interval * 3;
        let cid = controller_id.clone();

        // 3. Forward inner_rx → out_tx (outbound messages to controller)
        tokio::spawn({
            let out_tx = out_tx.clone();
            async move {
                while let Some(msg) = inner_rx.recv().await {
                    if out_tx.send(Ok(msg)).await.is_err() {
                        break;
                    }
                }
            }
        });

        // 4. Heartbeat task
        tokio::spawn({
            let inner_tx = inner_tx.clone();
            let registry = registry.clone();
            let cid = cid.clone();
            async move {
                let mut interval = tokio::time::interval(ping_interval);
                interval.tick().await; // skip first
                loop {
                    interval.tick().await;
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    if inner_tx.send(CenterMessage {
                        payload: Some(CenterPayload::Ping(Ping { timestamp: now_ms })),
                    }).await.is_err() {
                        tracing::info!(component = "fed_server", controller_id = %cid, "Heartbeat channel closed");
                        break;
                    }
                }
            }
        });

        // 5. Main message loop
        tokio::spawn({
            let registry = registry.clone();
            let cid = cid.clone();
            async move {
                let mut last_pong = std::time::Instant::now();

                loop {
                    match tokio::time::timeout(heartbeat_timeout, inbound.message()).await {
                        Err(_) => {
                            tracing::warn!(
                                component = "fed_server",
                                controller_id = %cid,
                                "Heartbeat timeout, marking offline"
                            );
                            registry.mark_offline(&cid);
                            break;
                        }
                        Ok(Err(e)) => {
                            tracing::info!(component = "fed_server", controller_id = %cid, error = %e, "Stream error");
                            registry.mark_offline(&cid);
                            break;
                        }
                        Ok(Ok(None)) => {
                            tracing::info!(component = "fed_server", controller_id = %cid, "Stream closed");
                            registry.mark_offline(&cid);
                            break;
                        }
                        Ok(Ok(Some(msg))) => {
                            registry.update_last_seen(&cid);
                            match msg.payload {
                                Some(CtrlPayload::Pong(_)) => {
                                    last_pong = std::time::Instant::now();
                                    let _ = last_pong; // used implicitly
                                }
                                Some(CtrlPayload::ListResponse(resp)) => {
                                    // Deliver to pending map (remove entry)
                                    if let Some(entry) = pending_lists.lock().unwrap().remove(&resp.request_id) {
                                        let _ = entry.sender.send(resp.keys.clone());
                                    }
                                    // Update aggregator
                                    aggregator.update_snapshot(&cid, resp.keys);
                                }
                                Some(CtrlPayload::CommandResponse(resp)) => {
                                    if let Some(sender) = pending_commands.lock().unwrap().remove(&resp.request_id) {
                                        let _ = sender.send(resp);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(out_rx)))
    }
}
```

- [ ] **Step 5: Update fed_sync/mod.rs**

`src/core/center/fed_sync/mod.rs`:
```rust
pub mod registry;
pub mod server;
```

- [ ] **Step 6: Run tests**

```bash
cargo test center::fed_sync 2>&1 | tail -10
```

Expected: registry tests pass, server compiles (no unit tests for server yet).

- [ ] **Step 7: Commit**

```bash
git add src/core/center/fed_sync/
git commit -m "feat: add center gRPC server and controller registry"
```

---

## Task 8: Resource Aggregator

**Files:**
- Create: `src/core/center/aggregator/mod.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(kind: &str, name: &str) -> ResourceKey {
        ResourceKey { kind: kind.to_string(), namespace: "default".to_string(),
            name: name.to_string(), resource_version: "1".to_string(),
            labels: Default::default(), annotations: Default::default() }
    }

    #[test]
    fn test_update_and_query() {
        let agg = ResourceAggregator::new();
        agg.update_snapshot("ctrl-1", vec![make_key("HTTPRoute", "r1")]);
        let all = agg.list_all_keys();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "r1");
    }

    #[test]
    fn test_offline_data_retained_and_evicted() {
        let agg = ResourceAggregator::new();
        agg.update_snapshot("ctrl-1", vec![make_key("Gateway", "g1")]);
        agg.mark_offline("ctrl-1");
        assert_eq!(agg.list_all_keys().len(), 1); // still there
        agg.evict_stale(0);                       // evict immediately
        assert_eq!(agg.list_all_keys().len(), 0);
    }

    #[test]
    fn test_query_by_cluster() {
        let agg = ResourceAggregator::new();
        let info1 = RegisterRequest { controller_id: "a/c1".to_string(),
            cluster: "cluster-a".to_string(), env: vec![], tag: vec![], supported_kinds: vec![] };
        let info2 = RegisterRequest { controller_id: "b/c2".to_string(),
            cluster: "cluster-b".to_string(), env: vec![], tag: vec![], supported_kinds: vec![] };
        agg.set_controller_info("a/c1", info1);
        agg.set_controller_info("b/c2", info2);
        agg.update_snapshot("a/c1", vec![make_key("HTTPRoute", "r1")]);
        agg.update_snapshot("b/c2", vec![make_key("HTTPRoute", "r2")]);
        let cluster_a = agg.list_keys_by_cluster("cluster-a");
        assert_eq!(cluster_a.len(), 1);
        assert_eq!(cluster_a[0].name, "r1");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test center::aggregator 2>&1 | tail -5
```

- [ ] **Step 3: Implement ResourceAggregator**

Full `src/core/center/aggregator/mod.rs`:

```rust
//! In-memory resource key aggregator.
//!
//! Stores per-controller snapshots. Snapshots are replaced on each ListResponse.
//! Offline entries are retained for `offline_evict_hours` before being dropped.

use crate::core::common::fed_sync::proto::{RegisterRequest, ResourceKey};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct ControllerSnapshot {
    info: RegisterRequest,
    keys: Vec<ResourceKey>,
    last_list_at: Option<Instant>,
    offline_since: Option<Instant>,
}

impl ControllerSnapshot {
    fn new(info: RegisterRequest) -> Self {
        Self { info, keys: vec![], last_list_at: None, offline_since: None }
    }
}

#[derive(Clone)]
pub struct ResourceAggregator {
    inner: Arc<RwLock<HashMap<String, ControllerSnapshot>>>,
}

impl ResourceAggregator {
    pub fn new() -> Self {
        Self { inner: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Called when controller registers (or reconnects)
    pub fn set_controller_info(&self, controller_id: &str, info: RegisterRequest) {
        let mut map = self.inner.write().unwrap();
        let snap = map.entry(controller_id.to_string())
            .or_insert_with(|| ControllerSnapshot::new(info.clone()));
        snap.info = info;
        snap.offline_since = None; // reconnect clears offline state
    }

    /// Full-replace snapshot for a controller. Called on each ListResponse.
    pub fn update_snapshot(&self, controller_id: &str, keys: Vec<ResourceKey>) {
        let mut map = self.inner.write().unwrap();
        if let Some(snap) = map.get_mut(controller_id) {
            snap.keys = keys;
            snap.last_list_at = Some(Instant::now());
        } else {
            // Controller connected but info not yet set (edge case); store anyway
            let mut snap = ControllerSnapshot::new(RegisterRequest {
                controller_id: controller_id.to_string(),
                ..Default::default()
            });
            snap.keys = keys;
            snap.last_list_at = Some(Instant::now());
            map.insert(controller_id.to_string(), snap);
        }
    }

    pub fn mark_offline(&self, controller_id: &str) {
        if let Some(snap) = self.inner.write().unwrap().get_mut(controller_id) {
            if snap.offline_since.is_none() {
                snap.offline_since = Some(Instant::now());
            }
        }
    }

    /// Evict controllers offline for longer than `hours`.
    pub fn evict_stale(&self, hours: u64) {
        let threshold = Duration::from_secs(hours * 3600);
        self.inner.write().unwrap().retain(|_, snap| {
            snap.offline_since.map(|s| s.elapsed() <= threshold).unwrap_or(true)
        });
    }

    /// All resource keys across all controllers (online + offline).
    pub fn list_all_keys(&self) -> Vec<ResourceKey> {
        self.inner.read().unwrap().values()
            .flat_map(|s| s.keys.clone())
            .collect()
    }

    /// Resource keys from controllers in a specific cluster.
    pub fn list_keys_by_cluster(&self, cluster: &str) -> Vec<ResourceKey> {
        self.inner.read().unwrap().values()
            .filter(|s| s.info.cluster == cluster)
            .flat_map(|s| s.keys.clone())
            .collect()
    }

    /// Summary of all known controllers (for Admin API).
    pub fn controller_summaries(&self) -> Vec<ControllerSummary> {
        self.inner.read().unwrap().values().map(|s| ControllerSummary {
            controller_id: s.info.controller_id.clone(),
            cluster: s.info.cluster.clone(),
            env: s.info.env.clone(),
            tag: s.info.tag.clone(),
            online: s.offline_since.is_none(),
            last_list_secs_ago: s.last_list_at.map(|t| t.elapsed().as_secs()),
            key_count: s.keys.len(),
        }).collect()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ControllerSummary {
    pub controller_id: String,
    pub cluster: String,
    pub env: Vec<String>,
    pub tag: Vec<String>,
    pub online: bool,
    /// Seconds elapsed since last successful list (None = never listed)
    pub last_list_secs_ago: Option<u64>,
    pub key_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(kind: &str, name: &str) -> ResourceKey {
        ResourceKey {
            kind: kind.to_string(), namespace: "default".to_string(),
            name: name.to_string(), resource_version: "1".to_string(),
            labels: Default::default(), annotations: Default::default(),
        }
    }

    #[test]
    fn test_update_and_query() {
        let agg = ResourceAggregator::new();
        agg.update_snapshot("ctrl-1", vec![make_key("HTTPRoute", "r1")]);
        let all = agg.list_all_keys();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "r1");
    }

    #[test]
    fn test_offline_data_retained_and_evicted() {
        let agg = ResourceAggregator::new();
        agg.update_snapshot("ctrl-1", vec![make_key("Gateway", "g1")]);
        agg.mark_offline("ctrl-1");
        assert_eq!(agg.list_all_keys().len(), 1);
        agg.evict_stale(0);
        assert_eq!(agg.list_all_keys().len(), 0);
    }

    #[test]
    fn test_query_by_cluster() {
        let agg = ResourceAggregator::new();
        let info1 = RegisterRequest { controller_id: "a/c1".to_string(),
            cluster: "cluster-a".to_string(), env: vec![], tag: vec![], supported_kinds: vec![] };
        let info2 = RegisterRequest { controller_id: "b/c2".to_string(),
            cluster: "cluster-b".to_string(), env: vec![], tag: vec![], supported_kinds: vec![] };
        agg.set_controller_info("a/c1", info1);
        agg.set_controller_info("b/c2", info2);
        agg.update_snapshot("a/c1", vec![make_key("HTTPRoute", "r1")]);
        agg.update_snapshot("b/c2", vec![make_key("HTTPRoute", "r2")]);
        let cluster_a = agg.list_keys_by_cluster("cluster-a");
        assert_eq!(cluster_a.len(), 1);
        assert_eq!(cluster_a[0].name, "r1");
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test center::aggregator 2>&1 | tail -10
```

Expected: all 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/center/aggregator/mod.rs
git commit -m "feat: add ResourceAggregator for center in-memory snapshot"
```

---

## Task 9: Scheduler

**Files:**
- Create: `src/core/center/scheduler/mod.rs`

**What it does:** Every `list_interval_secs`, iterates over all online controllers via `registry.online_senders()`, generates a UUID `request_id`, registers it in `server.pending_lists`, and sends a `ListRequest` down the stream. A background task cleans up timed-out entries from `pending_lists`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_send_list_request_registers_pending() {
        let registry = crate::core::center::fed_sync::registry::ControllerRegistry::new();
        let pending: super::super::fed_sync::server::PendingListMap =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        registry.register(
            "c1".to_string(),
            Default::default(),
            tx,
            "s1".to_string(),
        );

        send_list_requests(&registry, &pending).await;

        // A ListRequest should have been sent
        let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await.unwrap().unwrap();
        let request_id = match msg.payload {
            Some(crate::core::common::fed_sync::proto::center_message::Payload::ListRequest(r)) => r.request_id,
            _ => panic!("Expected ListRequest"),
        };
        // The request_id should be in pending map
        assert!(pending.lock().unwrap().contains_key(&request_id));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test center::scheduler 2>&1 | tail -5
```

- [ ] **Step 3: Implement Scheduler**

Full `src/core/center/scheduler/mod.rs`:

```rust
//! Periodic scheduler: sends ListRequest to all online controllers every list_interval_secs.

use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::core::center::config::CenterSyncConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::{PendingListMap, PendingListEntry};
use crate::core::common::fed_sync::proto::{
    center_message::Payload as CenterPayload,
    CenterMessage, ListRequest,
};

/// Send ListRequests to all currently online controllers, registering each in pending_lists.
pub async fn send_list_requests(
    registry: &ControllerRegistry,
    pending: &PendingListMap,
) {
    let senders = registry.online_senders();
    for (controller_id, tx) in senders {
        let request_id = Uuid::new_v4().to_string();
        let (resp_tx, _resp_rx) = oneshot::channel();

        pending.lock().unwrap().insert(request_id.clone(), PendingListEntry {
            sender: resp_tx,
            created_at: std::time::Instant::now(),
        });

        let msg = CenterMessage {
            payload: Some(CenterPayload::ListRequest(ListRequest {
                request_id: request_id.clone(),
                kinds: vec![],
            })),
        };

        if tx.send(msg).await.is_err() {
            tracing::warn!(
                component = "scheduler",
                controller_id = %controller_id,
                "Failed to send ListRequest (stream closed)"
            );
            pending.lock().unwrap().remove(&request_id);
        } else {
            tracing::debug!(
                component = "scheduler",
                controller_id = %controller_id,
                request_id = %request_id,
                "ListRequest sent"
            );
        }
    }
}

/// Run the scheduler loop. Exits when shutdown fires.
pub async fn run(
    registry: ControllerRegistry,
    pending: PendingListMap,
    config: CenterSyncConfig,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let list_interval = Duration::from_secs(config.list_interval_secs);
    let list_timeout = Duration::from_secs(config.list_timeout_secs);
    let mut interval = tokio::time::interval(list_interval);
    interval.tick().await; // skip immediate first tick

    // Background task: evict pending ListRequest entries that exceeded list_timeout.
    // Entries store `created_at: Instant`; any entry older than list_timeout is dropped.
    {
        let pending = pending.clone();
        tokio::spawn(async move {
            let mut gc_interval = tokio::time::interval(list_timeout);
            gc_interval.tick().await; // skip first
            loop {
                gc_interval.tick().await;
                pending.lock().unwrap().retain(|request_id, entry| {
                    let timed_out = entry.created_at.elapsed() >= list_timeout;
                    if timed_out {
                        tracing::warn!(
                            component = "scheduler",
                            request_id = %request_id,
                            "ListRequest timed out, dropping pending entry"
                        );
                    }
                    !timed_out
                });
            }
        });
    }

    loop {
        tokio::select! {
            _ = interval.tick() => {
                tracing::debug!(component = "scheduler", "Sending ListRequests to all online controllers");
                send_list_requests(&registry, &pending).await;
            }
            _ = shutdown.changed() => {
                tracing::info!(component = "scheduler", "Scheduler shutdown");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::common::fed_sync::proto::RegisterRequest;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // Default for RegisterRequest is defined in src/core/common/fed_sync/proto.rs

    #[tokio::test]
    async fn test_send_list_request_registers_pending() {
        let registry = ControllerRegistry::new();
        let pending: PendingListMap = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        registry.register("c1".to_string(), RegisterRequest::default(), tx, "s1".to_string());

        send_list_requests(&registry, &pending).await;

        let msg = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx.recv(),
        ).await.unwrap().unwrap();

        let request_id = match msg.payload {
            Some(CenterPayload::ListRequest(r)) => r.request_id,
            _ => panic!("Expected ListRequest"),
        };

        // Verify entry is in pending map with a recent timestamp
        let map = pending.lock().unwrap();
        let entry = map.get(&request_id).expect("request_id should be in pending map");
        assert!(entry.created_at.elapsed().as_secs() < 5);
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test center::scheduler 2>&1 | tail -10
```

Expected: test passes.

- [ ] **Step 5: Commit**

```bash
git add src/core/center/scheduler/mod.rs
git commit -m "feat: add center scheduler for periodic ListRequest dispatch"
```

---

## Task 10: Commander + Admin API

**Files:**
- Create: `src/core/center/commander/mod.rs`
- Create: `src/core/center/api/mod.rs`

**Commander** sends a `CommandRequest` to a specific controller and awaits `CommandResponse` (30s timeout) via the `pending_commands` map in the server.

**ResourceKey serialization:** prost-generated types do not impl `serde::Serialize`. Add a serializable DTO in `api/mod.rs`:

```rust
#[derive(serde::Serialize)]
struct ResourceKeyDto {
    kind: String,
    namespace: String,
    name: String,
    resource_version: String,
    labels: std::collections::HashMap<String, String>,
    annotations: std::collections::HashMap<String, String>,
}

impl From<crate::core::common::fed_sync::proto::ResourceKey> for ResourceKeyDto {
    fn from(k: crate::core::common::fed_sync::proto::ResourceKey) -> Self {
        Self {
            kind: k.kind, namespace: k.namespace, name: k.name,
            resource_version: k.resource_version,
            labels: k.labels, annotations: k.annotations,
        }
    }
}
```

Use `Json(keys.into_iter().map(ResourceKeyDto::from).collect::<Vec<_>>())` in the `/resources` handler instead of `Json(keys)`.

**Admin API** (axum) exposes:
- `GET /controllers` → list all controller summaries from aggregator
- `GET /clusters` → list distinct cluster names
- `GET /resources?cluster=&kind=` → filtered resource keys
- `POST /controllers/{id}/command` → dispatch command via commander

- [ ] **Step 1: Implement commander**

`src/core/center/commander/mod.rs`:

```rust
//! CommandDispatcher: sends CommandRequest to a specific controller and awaits response.

use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;
use anyhow::{anyhow, Result};

use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::PendingCommandMap;
use crate::core::common::fed_sync::proto::{
    center_message::Payload as CenterPayload,
    CenterMessage, CommandRequest, CommandResponse,
};

pub struct Commander {
    registry: ControllerRegistry,
    pending: PendingCommandMap,
    timeout: Duration,
}

impl Commander {
    pub fn new(registry: ControllerRegistry, pending: PendingCommandMap, timeout_secs: u64) -> Self {
        Self {
            registry,
            pending,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    pub async fn send_command(
        &self,
        controller_id: &str,
        command: crate::core::common::fed_sync::proto::command_request::Command,
    ) -> Result<CommandResponse> {
        let session = self.registry.get_session(controller_id)
            .ok_or_else(|| anyhow!("Controller {} not found or offline", controller_id))?;

        if session.offline_since.is_some() {
            return Err(anyhow!("Controller {} is offline", controller_id));
        }

        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel::<CommandResponse>();
        self.pending.lock().unwrap().insert(request_id.clone(), tx);

        let msg = CenterMessage {
            payload: Some(CenterPayload::Command(CommandRequest {
                request_id: request_id.clone(),
                command: Some(command),
            })),
        };

        session.stream_tx.send(msg).await
            .map_err(|_| {
                self.pending.lock().unwrap().remove(&request_id);
                anyhow!("Failed to send command: stream closed")
            })?;

        tokio::time::timeout(self.timeout, rx).await
            .map_err(|_| {
                self.pending.lock().unwrap().remove(&request_id);
                anyhow!("Command timed out after {}s", self.timeout.as_secs())
            })?
            .map_err(|_| anyhow!("Command response channel dropped"))
    }
}
```

- [ ] **Step 2: Implement Admin API**

`src/core/center/api/mod.rs`:

```rust
//! Admin HTTP API for edgion-center.
//!
//! Routes:
//!   GET  /controllers              → list all controller summaries
//!   GET  /clusters                 → list distinct cluster names
//!   GET  /resources                → list resource keys (query: cluster=, kind=)
//!   POST /controllers/{id}/reload  → send reload command

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::commander::Commander;
use crate::core::common::fed_sync::proto::command_request::Command;
use crate::core::common::fed_sync::proto::ReloadCommand;

#[derive(Clone)]
pub struct ApiState {
    pub aggregator: Arc<ResourceAggregator>,
    pub commander: Arc<Commander>,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/controllers", get(list_controllers))
        .route("/clusters", get(list_clusters))
        .route("/resources", get(list_resources))
        .route("/controllers/:id/reload", post(reload_controller))
        .with_state(state)
}

async fn list_controllers(State(state): State<ApiState>) -> impl IntoResponse {
    let summaries = state.aggregator.controller_summaries();
    Json(summaries)
}

async fn list_clusters(State(state): State<ApiState>) -> impl IntoResponse {
    let summaries = state.aggregator.controller_summaries();
    let mut clusters: Vec<String> = summaries.iter().map(|s| s.cluster.clone()).collect();
    clusters.sort();
    clusters.dedup();
    Json(clusters)
}

#[derive(Deserialize)]
struct ResourceQuery {
    cluster: Option<String>,
    kind: Option<String>,
}

async fn list_resources(
    State(state): State<ApiState>,
    Query(params): Query<ResourceQuery>,
) -> impl IntoResponse {
    let keys = match &params.cluster {
        Some(c) => state.aggregator.list_keys_by_cluster(c),
        None => state.aggregator.list_all_keys(),
    };
    let keys: Vec<ResourceKeyDto> = match &params.kind {
        Some(k) => keys.into_iter().filter(|key| &key.kind == k).map(ResourceKeyDto::from).collect(),
        None => keys.into_iter().map(ResourceKeyDto::from).collect(),
    };
    Json(keys)
}

async fn reload_controller(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.commander.send_command(&id, Command::Reload(ReloadCommand {})).await {
        Ok(resp) if resp.success => (StatusCode::OK, Json(serde_json::json!({"ok": true}))),
        Ok(resp) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": resp.message}))),
        Err(e) => {
            if e.to_string().contains("timed out") {
                (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"error": e.to_string()})))
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
            }
        }
    }
}
```

- [ ] **Step 3: Run cargo check**

```bash
cargo check --bin edgion-center 2>&1 | grep "error" | head -20
```

Fix any compile errors. Common issues: missing trait bounds, `serde::Serialize` not derived on `ResourceKey` (the proto-generated type — may need a wrapper).

- [ ] **Step 4: Commit**

```bash
git add src/core/center/commander/mod.rs src/core/center/api/mod.rs
git commit -m "feat: add center commander and admin API"
```

---

## Task 11: Wire Center Startup

**Files:**
- Modify: `src/core/center/cli/mod.rs`

**Goal:** Full startup sequence — create all components, start gRPC server and HTTP Admin API, run scheduler.

- [ ] **Step 1: Update cli/mod.rs with full startup**

Replace the stub `run()` in `src/core/center/cli/mod.rs`:

```rust
use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tokio::sync::watch;

use crate::core::center::aggregator::ResourceAggregator;
use crate::core::center::api::{router, ApiState};
use crate::core::center::commander::Commander;
use crate::core::center::config::CenterConfig;
use crate::core::center::fed_sync::registry::ControllerRegistry;
use crate::core::center::fed_sync::server::FederationGrpcServer;
use crate::core::center::scheduler;
use crate::core::common::fed_sync::proto::federation_sync_server::FederationSyncServer;

#[derive(Parser, Debug)]
#[command(name = "edgion-center", version, about = "Edgion Federated Center")]
pub struct EdgionCenterCli {
    #[arg(short = 'c', long, default_value = "config/edgion-center.toml")]
    pub config_file: String,
}

impl EdgionCenterCli {
    pub fn parse_args() -> Self { Self::parse() }

    pub async fn run(&self) -> Result<()> {
        let content = std::fs::read_to_string(&self.config_file).unwrap_or_default();
        let config: CenterConfig = toml::from_str(&content).unwrap_or_default();

        let registry = ControllerRegistry::new();
        let aggregator = Arc::new(ResourceAggregator::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let grpc_server = FederationGrpcServer::new(
            registry.clone(),
            aggregator.clone(),
            config.sync.clone(),
        );
        let pending_lists = grpc_server.pending_lists.clone();
        let pending_commands = grpc_server.pending_commands.clone();

        let commander = Arc::new(Commander::new(
            registry.clone(),
            pending_commands,
            config.sync.command_timeout_secs,
        ));

        let api_state = ApiState { aggregator: aggregator.clone(), commander };
        let http_addr: std::net::SocketAddr = config.server.http_addr.parse()?;
        let grpc_addr: std::net::SocketAddr = config.server.grpc_addr.parse()?;

        tracing::info!(component = "center", grpc_addr = %grpc_addr, http_addr = %http_addr, "Starting edgion-center");

        // gRPC server
        let grpc_handle = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(FederationSyncServer::new(grpc_server))
                .serve(grpc_addr)
        );

        // HTTP Admin API
        let http_handle = tokio::spawn(
            axum::serve(
                tokio::net::TcpListener::bind(http_addr).await?,
                router(api_state),
            ).into_future()
        );

        // Scheduler
        let sched_handle = tokio::spawn(scheduler::run(
            registry.clone(),
            pending_lists,
            config.sync.clone(),
            shutdown_rx.clone(),
        ));

        // Wait for any task to exit or Ctrl-C
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(component = "center", "Ctrl-C received, shutting down");
                let _ = shutdown_tx.send(true);
            }
            r = grpc_handle => { tracing::error!(component = "center", "gRPC server exited: {:?}", r); }
            r = http_handle => { tracing::error!(component = "center", "HTTP server exited: {:?}", r); }
            r = sched_handle => { tracing::error!(component = "center", "Scheduler exited: {:?}", r); }
        }

        Ok(())
    }
}
```

- [ ] **Step 2: Add logging init to bin**

In `src/bin/edgion_center.rs`, add a minimal tracing init before `cli.run()`:

```rust
tracing_subscriber::fmt()
    .with_env_filter(
        tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("info".parse().unwrap())
    )
    .init();
```

- [ ] **Step 3: Compile check**

```bash
cargo build --bin edgion-center 2>&1 | grep "error" | head -20
```

Fix all compile errors.

- [ ] **Step 4: Smoke test**

In one terminal:
```bash
cargo run --bin edgion-center -- -c /dev/null &
sleep 1
curl -s http://localhost:5810/controllers | python3 -m json.tool
curl -s http://localhost:5810/clusters
kill %1
```

Expected: `[]` for both endpoints (no controllers connected yet).

- [ ] **Step 5: Run all tests**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 6: Final commit**

```bash
git add src/core/center/cli/mod.rs src/bin/edgion_center.rs
git commit -m "feat: wire full edgion-center startup (gRPC + HTTP + scheduler)"
```

---

## Task 12: Run Full Test Suite + Cleanup

- [ ] **Step 1: Run cargo check on all targets**

```bash
cargo check --all-targets 2>&1 | grep "^error" | head -20
```

Expected: zero errors.

- [ ] **Step 2: Run all unit tests**

```bash
cargo test --all 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 3: Run linter**

```bash
cargo clippy --all-targets 2>&1 | grep "^error" | head -20
```

Fix any hard errors (warnings are OK).

- [ ] **Step 4: Run formatter check**

```bash
cargo fmt --all -- --check
```

If formatting issues: `cargo fmt --all`

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "chore: format and clippy fixes for federated center"
```
