# Edgion Plugin Development Guide

> Quick reference for adding new EdgionPlugins. Follow the checklist, copy the patterns, ship fast.
>
> **TODO (2026-02-25): Medium Improvement**
> - [ ] Add UpstreamResponseFilter / UpstreamResponseBodyFilter minimal development example (not just trait signature)
> - [ ] Add ConditionalFilter condition configuration guide (how users configure `skip`/`run` conditions in YAML)
> - [ ] Clarify plugin preparse flow: where preparse happens (EdgionPluginsHandler vs HTTPRouteHandler), controller-side vs gateway-side
> - [ ] Add cross-plugin data passing patterns via `ctx_map` (e.g., RealIp sets `client_ip` → RateLimit reads `client_ip`)

## Directory Structure

```
src/
├── core/plugins/
│   ├── edgion_plugins/                          # All plugin implementations live here
│   │   ├── common/                              # Shared utilities across plugins
│   │   │   ├── mod.rs
│   │   │   └── http_client.rs                   # Global reqwest::Client singleton (get_http_client())
│   │   ├── <your_plugin>/                       # Your new plugin directory
│   │   │   ├── mod.rs                           # pub use plugin::YourPlugin;
│   │   │   └── plugin.rs                        # impl RequestFilter / UpstreamResponseFilter
│   │   └── mod.rs                               # Register: pub mod your_plugin; pub use ...;
│   └── plugin_runtime/                          # Core runtime (usually don't touch)
│       ├── traits/
│       │   ├── session.rs                       # PluginSession trait (read/write request & response)
│       │   ├── request_filter.rs                # RequestFilter trait (async, before upstream)
│       │   ├── upstream_response_filter.rs      # UpstreamResponseFilter trait (sync, after upstream headers)
│       │   ├── upstream_response.rs             # UpstreamResponse trait (async, after upstream)
│       │   └── upstream_response_body_filter.rs # UpstreamResponseBodyFilter trait (sync, per body chunk)
│       ├── runtime.rs                           # Plugin instantiation: create_*_from_edgion()
│       ├── conditional_filter.rs                # Auto-wraps all plugins with skip/run conditions
│       └── log.rs                               # PluginLog (goes into access log)
└── types/
    ├── resources/edgion_plugins/
    │   ├── plugin_configs/
    │   │   ├── mod.rs                           # Export your config
    │   │   └── <your_plugin>.rs                 # YourPluginConfig struct
    │   ├── edgion_plugin.rs                     # EdgionPlugin enum: add your variant
    │   ├── entry.rs                             # PluginEntry types (don't touch)
    │   └── mod.rs                               # Re-export your config types
    ├── filters.rs                               # PluginRunningResult enum
    └── common/key_accessor.rs                   # KeyGet/KeySet unified accessors
```

## New Plugin Checklist

Adding a plugin touches **6 files** (create 3, modify 3):

### Create

1. **Config** — `src/types/resources/edgion_plugins/plugin_configs/<your_plugin>.rs`
2. **Plugin mod** — `src/core/plugins/edgion_plugins/<your_plugin>/mod.rs`
3. **Plugin impl** — `src/core/plugins/edgion_plugins/<your_plugin>/plugin.rs`

### Modify

4. **EdgionPlugin enum** — `src/types/resources/edgion_plugins/edgion_plugin.rs`
5. **Runtime registration** — `src/core/plugins/plugin_runtime/runtime.rs`
6. **Module exports** — multiple `mod.rs` files (see Step 6 below)

## Step-by-Step

### Step 1: Define Config

`src/types/resources/edgion_plugins/plugin_configs/your_plugin.rs`

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct YourPluginConfig {
    /// Required field — describe clearly
    pub some_field: String,

    /// Optional with default
    #[serde(default = "default_timeout")]
    pub timeout: u64,

    // === Runtime fields (populated by controller, not user-configurable) ===
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_secret: Option<String>,

    // === Validation cache ===
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

fn default_timeout() -> u64 { 3 }

impl Default for YourPluginConfig { /* ... */ }

impl YourPluginConfig {
    /// Return validation error if config is invalid.
    /// Called during preparse for status reporting.
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }
}
```

Key rules:
- `#[serde(rename_all = "camelCase")]` — YAML/JSON uses camelCase
- `#[schemars(skip)]` on runtime-only fields — keep them out of CRD schema
- Provide `get_validation_error()` if config can be structurally invalid
- Comments in English

### Step 2: Export Config

`src/types/resources/edgion_plugins/plugin_configs/mod.rs` — add:

```rust
mod your_plugin;
pub use your_plugin::YourPluginConfig;
```

`src/types/resources/edgion_plugins/mod.rs` — add to re-exports:

```rust
pub use plugin_configs::YourPluginConfig;
```

### Step 3: Add to EdgionPlugin Enum

`src/types/resources/edgion_plugins/edgion_plugin.rs`:

```rust
pub enum EdgionPlugin {
    // ... existing variants ...

    /// Your plugin description
    YourPlugin(YourPluginConfig),
}

impl EdgionPlugin {
    pub fn type_name(&self) -> &'static str {
        match self {
            // ... existing ...
            EdgionPlugin::YourPlugin(_) => "YourPlugin",
        }
    }
}
```

### Step 4: Implement Plugin

`src/core/plugins/edgion_plugins/your_plugin/mod.rs`:

```rust
mod plugin;
pub use plugin::YourPlugin;
```

`src/core/plugins/edgion_plugins/your_plugin/plugin.rs`:

```rust
use async_trait::async_trait;
use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::YourPluginConfig;

pub struct YourPlugin {
    name: String,
    config: YourPluginConfig,
}

impl YourPlugin {
    pub fn new(config: &YourPluginConfig) -> Self {
        Self {
            name: "YourPlugin".to_string(),
            config: config.clone(),
        }
    }
}

#[async_trait]
impl RequestFilter for YourPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        // Your logic here
        log.push("OK; ");
        PluginRunningResult::GoodNext
    }
}
```

### Step 5: Register in Runtime

`src/core/plugins/plugin_runtime/runtime.rs`:

Add import at top:

```rust
use crate::core::plugins::edgion_plugins::your_plugin::YourPlugin;
```

Add to `create_request_filter_from_edgion()`:

```rust
EdgionPlugin::YourPlugin(config) => Some(Box::new(YourPlugin::new(config))),
```

If your plugin has validation, add to `get_plugin_validation_error()`:

```rust
EdgionPlugin::YourPlugin(config) => config.get_validation_error().map(|s| s.to_string()),
```

### Step 6: Export Plugin

`src/core/plugins/edgion_plugins/mod.rs` — add:

```rust
pub mod your_plugin;
pub use your_plugin::YourPlugin;
```

## Plugin Stages

Choose the right trait based on when your plugin runs:

| Trait | When | Async | Typical Use |
|-------|------|-------|-------------|
| `RequestFilter` | Before upstream | Yes | Auth, rate limit, rewrite, redirect, mock |
| `UpstreamResponseFilter` | After upstream headers | No | Response header modification |
| `UpstreamResponseBodyFilter` | Per body chunk | No | Bandwidth limiting, body inspection |
| `UpstreamResponse` | After upstream (full) | Yes | (Reserved for future) |

Most plugins are `RequestFilter`. Pick the narrowest stage that fits.

## PluginRunningResult

Return value from your plugin's `run_*` method:

| Result | Meaning | Next plugins run? |
|--------|---------|-------------------|
| `GoodNext` | Success, continue | Yes |
| `Nothing` | No-op, continue | Yes |
| `ErrTerminateRequest` | Hard stop (you already wrote response) | No |
| `ErrResponse { status, body }` | Return error (runtime writes response) | No |

For `ErrTerminateRequest`, you must write the response yourself via `session.write_response_header()` + `session.write_response_body()` + `session.shutdown()`.

## PluginSession Key APIs

Read request:

```rust
session.header_value("Authorization")     // Single header
session.request_headers()                  // All headers
session.get_path()                         // "/api/v1/users"
session.get_query_param("page")            // Query param
session.get_cookie("session_id")           // Cookie
session.method()                           // "GET"
session.client_addr()                      // TCP direct IP
session.remote_addr()                      // Real client IP (after RealIp plugin)
session.get_path_param("uid")              // Route param ("/api/:uid")
```

Modify request (for upstream):

```rust
session.set_request_header("X-User-ID", "123")
session.append_request_header("X-Forwarded-For", ip)
session.remove_request_header("Authorization")
session.set_upstream_uri("/new/path")
session.set_upstream_host("backend.internal")
session.set_upstream_method("POST")
```

Context variables (pass data between plugins):

```rust
session.set_ctx_var("my_plugin_data", "value")
session.get_ctx_var("my_plugin_data")
session.remove_ctx_var("my_plugin_data")
```

Write response (for termination):

```rust
session.write_response_header(Box::new(resp), false).await?;
session.write_response_body(Some(Bytes::from("body")), true).await?;
session.shutdown().await;
```

Unified accessor (for generic key sources — use when your plugin needs configurable input sources):

```rust
session.key_get(&KeyGet::Header { name: "X-Key".into() })  // From header
session.key_get(&KeyGet::Query { name: "key".into() })      // From query
session.key_get(&KeyGet::Cookie { name: "key".into() })     // From cookie
session.key_get(&KeyGet::Ctx { name: "var".into() })        // From ctx var
session.key_get(&KeyGet::ClientIp)                           // Client IP
```

## PluginLog

PluginLog goes into the access log. Goal: **one access log line tells the full story**.

Rules:
- **Short + useful** — every byte counts (fixed 100-byte buffer by default)
- Log **outcomes**, not internal steps
- Use semicolons as separators: `"OK u=jack; "`, `"FAIL; "`, `"Skip; "`
- Always end entries with `"; "` for readability

```rust
// Good — concise, tells what happened
log.push("OK u=jack; ");
log.push("Deny ip=1.2.3.4; ");
log.push("Rate 429 k=user:123; ");
log.push("FAIL; ");
log.push("Anon; ");

// Bad — too verbose, wastes buffer
log.push("Starting authentication process for user");
log.push("Successfully verified JWT token with algorithm RS256");
```

For detailed debugging, use `tracing::debug!()` — it goes to system log, not access log.

## Common Utilities

`src/core/plugins/edgion_plugins/common/` — shared code lives here.

### HTTP Client

For plugins calling external services (IdP, auth service, webhook, etc.):

```rust
use crate::core::plugins::edgion_plugins::common::http_client::get_http_client;

let client = get_http_client();  // Global singleton, connection pooling
let resp = client.get(&url)
    .timeout(Duration::from_secs(self.config.timeout))  // Per-request override
    .send()
    .await?;
```

Features: connection pooling (32/host), no auto-redirect, 10s default timeout, rustls TLS.

### Hop-by-hop Header Filtering

```rust
use crate::core::plugins::edgion_plugins::common::http_client::is_hop_by_hop;

if !is_hop_by_hop(header_name) {
    // Safe to forward
}
```

### Adding to Common

If you find code that:
- Is used by **2+ plugins** with identical logic
- Is a self-contained utility (not plugin-specific business logic)
- Would be awkward to duplicate

Then extract it to `common/`. Add a new file, export in `common/mod.rs`.

## Architecture Notes

### No Consumer Model

Unlike Kong/APISIX, Edgion does **not** have a "Consumer" abstraction. In Kong, plugins are associated with Consumers (end users/applications), and authentication plugins look up Consumers by credentials.

In Edgion, `EdgionPlugin` instances are **directly referenced by routes** (via `EdgionPlugins` CRD). Authentication credentials (API keys, HMAC secrets, JWT keys, etc.) are stored in **Kubernetes Secrets** and referenced via `SecretObjectReference`. There is no separate Consumer entity — the plugin config itself defines who is authorized and what headers to set for upstream.

This means:
- **No `/consumers` API** — credentials are managed as K8s Secrets
- **No consumer-to-plugin association** — plugins bind to routes, not consumers
- **Upstream identity** is conveyed via request headers set by the plugin (e.g., `X-Consumer-Username`, `claims_to_headers`)
- **Per-key metadata** (like in `KeyAuth`) is stored alongside the key in the Secret, not in a separate Consumer object

When adapting designs from Kong/APISIX plugins, replace any Consumer-related flows with direct Secret-based credential lookup.

## Coding Principles

1. **Comments in English** — all code comments, doc comments, and log messages

2. **Extract to common when appropriate** — if logic is reusable across plugins, put it in `common/`. Don't force it; 2+ consumers is the threshold.

3. **TODO for deferred work** — if something can't be done now, leave a clear TODO:

```rust
// TODO: support external session storage (Redis) for cross-instance sharing
// TODO: add JWKS cache size limit
```

4. **Conditional filter is automatic** — every plugin gets `skip`/`run` condition support for free via `ConditionalRequestFilter` wrapping in `runtime.rs`. You don't need to handle this in your plugin.

5. **Secrets via K8s Secret** — never hardcode secrets. Use `SecretObjectReference` in config, controller resolves to `resolved_*` fields. At runtime, use `get_secret()` as fallback:

```rust
use crate::core::conf_mgr::sync_runtime::resource_processor::get_secret;
let secret = get_secret(Some(namespace), &secret_ref.name);
```

6. **Tests use MockPluginSession** — the `PluginSession` trait has `#[cfg_attr(test, mockall::automock)]`, so you get `MockPluginSession` for free:

```rust
#[cfg(test)]
mod tests {
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;

    #[tokio::test]
    async fn test_basic() {
        let mut mock = MockPluginSession::new();
        mock.expect_header_value().returning(|_| None);
        mock.expect_method().returning(|| "GET".to_string());
        // ... set up expectations, call plugin, assert result
    }
}
```

7. **Config validation** — implement `get_validation_error()` on your config if it can be structurally invalid. The error surfaces in the EdgionPlugins CRD status. Register it in `runtime.rs` → `get_plugin_validation_error()`.

## Quick Copy Template

Minimum viable plugin (request stage):

```bash
# Create plugin directory
mkdir -p src/core/plugins/edgion_plugins/your_plugin
```

Then create the 3 files (config, mod.rs, plugin.rs), modify the 3 registration points (enum, runtime, exports), and you're done. Use existing plugins as reference — `mock` is the simplest, `jwt_auth` is a full-featured auth example.
