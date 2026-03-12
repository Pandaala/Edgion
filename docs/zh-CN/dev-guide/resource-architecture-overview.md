# Resource Architecture Overview

This document provides a comprehensive overview of Edgion's resource architecture, explaining how Kubernetes resources (like HTTPRoute, GRPCRoute, etc.) flow through the system from YAML definitions to runtime components.

## Architecture Diagram

```mermaid
graph TD
    YAML[Resource YAML/JSON] -->|Deserialized| ResourceType[Resource Type Definition]
    ResourceType -->|Implements| ResourceMeta[ResourceMeta Trait]
    ResourceType -->|Registered in| ResourceKind[ResourceKind Enum]
    
    ResourceKind -->|Proto Definition| ProtoDef[config_sync.proto]
    ProtoDef -->|gRPC Service| Server[ConfigServer]
    ProtoDef -->|gRPC Client| Client[ConfigClient]
    
    Server -->|Watch/List API| ServerCache[ServerCache T]
    Client -->|Subscribe| ClientCache[ClientCache T]
    
    ServerCache -->|Apply Changes| EventDispatch[Server Event Dispatch]
    ClientCache -->|Apply Changes| ClientEventDispatch[Client Event Dispatch]
    
    EventDispatch -->|Validate & Store| Storage[(Resource Storage)]
    ClientEventDispatch -->|Process| Handler[Resource Handlers]
    
    Handler -->|Update| RouteManager[Route Manager]
    Handler -->|Update| ServiceStore[Service Store]
    Handler -->|Update| PluginStore[Plugin Store]
```

## System Components

### 1. Resource Type Definition

**Location**: `src/types/resources/`

Each Kubernetes resource is defined as a Rust struct using the `kube::CustomResource` derive macro or standard Kubernetes API types.

**Example Structure** (using GRPCRoute):

```rust
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "GRPCRoute",
    plural = "grpcroutes",
    namespaced
)]
pub struct GRPCRouteSpec {
    pub parent_refs: Option<Vec<ParentReference>>,
    pub hostnames: Option<Vec<String>>,
    pub rules: Option<Vec<GRPCRouteRule>>,
}
```

**Key Features**:
- Serialization/deserialization support via serde
- JSON schema generation for validation
- Runtime-only fields marked with `#[serde(skip)]`
- Pre-parsing hooks for computed fields

### 2. ResourceMeta Trait

**Location**: `src/types/resource_meta_traits/traits.rs`

The `ResourceMeta` trait provides a unified interface for all resource types:

```rust
pub trait ResourceMeta: DeserializeOwned + Send + Sync + 'static {
    fn get_version(&self) -> u64;
    fn resource_kind() -> ResourceKind;
    fn kind_name() -> &'static str;
    fn key_name(&self) -> String;
    fn pre_parse(&mut self);
}
```

**Purpose**:
- **Version tracking**: For optimistic concurrency control
- **Type identification**: For routing and dispatching events
- **Unique keys**: For cache storage (namespace/name format)
- **Pre-parsing**: For populating runtime-only fields after deserialization

**Implementation Example**:

```rust
impl ResourceMeta for GRPCRoute {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::GRPCRoute
    }
    
    fn kind_name() -> &'static str {
        "GRPCRoute"
    }
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
    
    fn pre_parse(&mut self) {
        // Parse extension refs, timeouts, etc.
        self.preparse();
        self.parse_timeouts();
    }
}
```

### 3. ResourceKind Enum

**Location**: `src/types/resource_kind.rs`

A central enum that identifies all resource types in the system:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ::prost::Enumeration)]
#[repr(i32)]
pub enum ResourceKind {
    Unspecified = 0,
    GatewayClass = 1,
    EdgionGatewayConfig = 2,
    Gateway = 3,
    HTTPRoute = 4,
    Service = 5,
    EndpointSlice = 6,
    EdgionTls = 7,
    Secret = 8,
    EdgionPlugins = 9,
    GRPCRoute = 10,
}
```

**Purpose**:
- Used in protobuf definitions for gRPC communication
- Enables type-safe dispatching in event handlers
- Supports dynamic type detection from YAML/JSON content

### 4. Protocol Buffer Definition

**Location**: `src/core/conf_sync/proto/config_sync.proto`

Defines the gRPC service for configuration synchronization:

```protobuf
service ConfigSync {
    rpc List(ListRequest) returns (ListResponse);
    rpc Watch(WatchRequest) returns (stream WatchResponse);
    rpc GetBaseConf(GetBaseConfRequest) returns (GetBaseConfResponse);
}

enum ResourceKind {
    RESOURCE_KIND_UNSPECIFIED = 0;
    RESOURCE_KIND_GATEWAY_CLASS = 1;
    // ... other kinds ...
    RESOURCE_KIND_GRPC_ROUTE = 10;
}
```

**Operations**:
- **List**: Fetch all resources of a specific kind
- **Watch**: Stream updates for resources (Add/Update/Delete)
- **GetBaseConf**: Fetch base configuration (GatewayClass, Gateway, EdgionGatewayConfig)

### 5. Server-Side Components

#### ConfigServer

**Location**: `src/core/conf_sync/conf_server/config_server.rs`

The server-side cache that stores all resources:

```rust
pub struct ConfigServer {
    pub base_conf: RwLock<GatewayBaseConf>,
    pub routes: ServerCache<HTTPRoute>,
    pub grpc_routes: ServerCache<GRPCRoute>,
    pub services: ServerCache<Service>,
    pub endpoint_slices: ServerCache<EndpointSlice>,
    pub edgion_tls: ServerCache<EdgionTls>,
    pub edgion_plugins: ServerCache<EdgionPlugins>,
    pub secrets: ServerCache<Secret>,
}
```

Each `ServerCache<T>` provides:
- Thread-safe storage with `RwLock`
- Version tracking for optimistic concurrency
- Event streaming to watching clients
- Ready state management

#### Server Event Dispatch

**Location**: `src/core/conf_sync/conf_server/event_dispatch.rs`

Handles incoming resource changes and applies them to caches:

```rust
fn apply_resource_change_with_resource_type(
    &self,
    change: ResourceChange,  // Add, Update, or Delete
    resource_type: ResourceKind,
    data: String,
) {
    match resource_type {
        ResourceKind::GRPCRoute => {
            if let Ok(resource) = serde_yaml::from_str::<GRPCRoute>(&data) {
                // Validate parent gateway references
                // Apply change to cache
                Self::execute_change_on_cache::<GRPCRoute>(change, &self.grpc_routes, resource);
            }
        }
        // ... other resource types ...
    }
}
```

**Validation Steps**:
1. Deserialize YAML/JSON to typed resource
2. Validate references (e.g., parent gateways must exist)
3. Apply change to appropriate cache
4. Notify watching clients

### 6. Client-Side Components

#### ConfigClient

**Location**: `src/core/conf_sync/conf_client/config_client.rs`

The client-side cache that subscribes to server updates:

```rust
pub struct ConfigClient {
    gateway_class_key: String,
    pub base_conf: RwLock<Option<GatewayBaseConf>>,
    routes: ClientCache<HTTPRoute>,
    grpc_routes: ClientCache<GRPCRoute>,
    services: ClientCache<Service>,
    endpoint_slices: ClientCache<EndpointSlice>,
    edgion_tls: ClientCache<EdgionTls>,
    edgion_plugins: ClientCache<EdgionPlugins>,
    secrets: ClientCache<Secret>,
}
```

Each `ClientCache<T>` provides:
- Local resource storage
- Event handlers for Add/Update/Delete
- Integration with domain-specific processors (RouteManager, ServiceStore, etc.)

#### Client Event Dispatch

**Location**: `src/core/conf_sync/conf_client/config_client.rs`

Processes incoming events from the server:

```rust
impl ConfigClientEventDispatcher for ConfigClient {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        _resource_version: Option<u64>,
    ) {
        match resource_type {
            ResourceKind::GRPCRoute => {
                if let Ok(resource) = serde_yaml::from_str::<GRPCRoute>(&data) {
                    Self::apply_change_to_cache(&self.grpc_routes, change, resource);
                }
            }
            // ... other resource types ...
        }
    }
}
```

### 7. Resource Handlers

**Purpose**: Process resource changes and update runtime components

**Examples**:
- **RouteManager** (`src/core/routes/`): Manages routing tables for HTTPRoute and GRPCRoute
- **ServiceStore** (`src/core/backends/`): Tracks Service resources
- **EpSliceHandler** (`src/core/backends/`): Manages endpoint slices for load balancing
- **PluginStore** (`src/core/plugins/`): Handles plugin configurations

**Integration**:

```rust
// In ConfigClient::new()
let routes_cache = ClientCache::new(gateway_class_key, client_id, client_name);
let route_handler = create_route_manager_handler();
routes_cache.set_conf_processor(route_handler);
```

When a route change occurs:
1. Cache receives the event
2. Cache calls the registered handler
3. Handler updates its internal state (e.g., routing table)
4. Gateway proxy uses updated state for request routing

## Data Flow

### Resource Addition Flow

```mermaid
sequenceDiagram
    participant K8s as Kubernetes API
    participant Loader as Config Loader
    participant Server as ConfigServer
    participant Client as ConfigClient
    participant Handler as Resource Handler
    participant Proxy as Gateway Proxy
    
    K8s->>Loader: Watch GRPCRoute
    Loader->>Server: Add(GRPCRoute)
    Server->>Server: Validate & Store
    Server->>Client: Watch Event (Add)
    Client->>Client: Deserialize
    Client->>Handler: Process Add
    Handler->>Handler: Update Routing Table
    
    Note over Proxy: Next Request
    Proxy->>Handler: Match Route
    Handler-->>Proxy: Route Rule
    Proxy->>Proxy: Apply Rule & Proxy
```

### Resource Update Flow

```mermaid
sequenceDiagram
    participant K8s as Kubernetes API
    participant Loader as Config Loader
    participant Server as ConfigServer
    participant Client as ConfigClient
    participant Handler as Resource Handler
    
    K8s->>Loader: Watch GRPCRoute (Modified)
    Loader->>Server: Update(GRPCRoute, new_version)
    Server->>Server: Version Check
    Server->>Server: Update Cache
    Server->>Client: Watch Event (Update)
    Client->>Client: Deserialize
    Client->>Handler: Process Update
    Handler->>Handler: Update Routing Table
```

### Resource Deletion Flow

```mermaid
sequenceDiagram
    participant K8s as Kubernetes API
    participant Loader as Config Loader
    participant Server as ConfigServer
    participant Client as ConfigClient
    participant Handler as Resource Handler
    
    K8s->>Loader: Watch GRPCRoute (Deleted)
    Loader->>Server: Delete(GRPCRoute)
    Server->>Server: Remove from Cache
    Server->>Client: Watch Event (Delete)
    Client->>Client: Remove from Cache
    Client->>Handler: Process Delete
    Handler->>Handler: Remove from Routing Table
```

## How to Add a New Resource Type

Follow these steps to add support for a new Kubernetes resource type:

### Step 1: Define Resource Type

Create a new file `src/types/resources/your_resource.rs`:

```rust
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const YOUR_RESOURCE_GROUP: &str = "gateway.networking.k8s.io";
pub const YOUR_RESOURCE_KIND: &str = "YourResource";

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "YourResource",
    plural = "yourresources",
    namespaced
)]
pub struct YourResourceSpec {
    // Your fields here
}
```

Export in `src/types/resources/mod.rs`:

```rust
pub mod your_resource;
pub use self::your_resource::*;
```

### Step 2: Implement ResourceMeta Trait

Create `src/types/resource_meta_traits/your_resource.rs`:

```rust
use crate::types::resource_kind::ResourceKind;
use crate::types::resources::YourResource;
use super::traits::{extract_version, ResourceMeta};

impl ResourceMeta for YourResource {
    fn get_version(&self) -> u64 {
        extract_version(&self.metadata)
    }
    
    fn resource_kind() -> ResourceKind {
        ResourceKind::YourResource
    }
    
    fn kind_name() -> &'static str {
        "YourResource"
    }
    
    fn key_name(&self) -> String {
        if let Some(namespace) = &self.metadata.namespace {
            format!("{}/{}", namespace, self.metadata.name.as_deref().unwrap_or(""))
        } else {
            self.metadata.name.as_deref().unwrap_or("").to_string()
        }
    }
    
    fn pre_parse(&mut self) {
        // Optional: parse runtime-only fields
    }
}
```

Export in `src/types/resource_meta_traits/mod.rs`:

```rust
mod your_resource;
```

### Step 3: Update ResourceKind Enum

Edit `src/types/resource_kind.rs`:

```rust
pub enum ResourceKind {
    // ... existing variants ...
    YourResource = 11,  // Next available number
}

impl ResourceKind {
    pub fn from_kind_name(kind_str: &str) -> Option<Self> {
        match kind_str {
            // ... existing cases ...
            "YourResource" => Some(ResourceKind::YourResource),
            _ => None,
        }
    }
}
```

### Step 4: Update Protocol Buffer

Edit `src/core/conf_sync/proto/config_sync.proto`:

```protobuf
enum ResourceKind {
    // ... existing ...
    RESOURCE_KIND_YOUR_RESOURCE = 11;
}
```

Rebuild to regenerate proto bindings:

```bash
cargo build
```

### Step 5: Update Server Cache

Edit `src/core/conf_sync/conf_server/config_server.rs`:

```rust
pub enum ResourceItem {
    // ... existing ...
    YourResource(YourResource),
}

pub struct ConfigServer {
    // ... existing caches ...
    pub your_resources: ServerCache<YourResource>,
}

impl ConfigServer {
    pub fn new(base_conf: GatewayBaseConf) -> Self {
        Self {
            // ... existing caches ...
            your_resources: ServerCache::new(200),
        }
    }
    
    pub fn list(&self, kind: &ResourceKind) -> Result<ListDataSimple, String> {
        let (data_json, resource_version) = match kind {
            // ... existing cases ...
            ResourceKind::YourResource => {
                let list_data = self.your_resources.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize YourResource data: {}", e))?;
                (json, list_data.resource_version)
            }
            // ...
        };
        // ...
    }
}
```

### Step 6: Update Server Event Dispatch

Edit `src/core/conf_sync/conf_server/event_dispatch.rs`:

```rust
impl ConfigServer {
    fn apply_resource_change_with_resource_type(
        &self,
        change: ResourceChange,
        resource_type: ResourceKind,
        data: String,
    ) {
        match resource_type {
            // ... existing cases ...
            ResourceKind::YourResource => {
                if let Ok(resource) = serde_yaml::from_str::<YourResource>(&data) {
                    tracing::info!("Applying YourResource resource change");
                    Self::execute_change_on_cache::<YourResource>(
                        change, 
                        &self.your_resources, 
                        resource
                    );
                }
            }
            // ...
        }
    }
}

impl ConfigServerEventDispatcher for ConfigServer {
    fn enable_version_fix_mode(&self) {
        // ... existing ...
        self.your_resources.enable_version_fix_mode();
    }

    fn set_ready(&self) {
        // ... existing ...
        self.your_resources.set_ready();
    }
}
```

### Step 7: Update Client Cache

Edit `src/core/conf_sync/conf_client/config_client.rs`:

```rust
pub struct ConfigClient {
    // ... existing ...
    your_resources: ClientCache<YourResource>,
}

impl ConfigClient {
    pub fn new(gateway_class_key: String, client_id: String, client_name: String) -> Self {
        let your_resources_cache = ClientCache::new(
            gateway_class_key.clone(), 
            client_id.clone(), 
            client_name.clone()
        );
        
        // Optional: Register handler
        // let handler = create_your_resource_handler();
        // your_resources_cache.set_conf_processor(handler);
        
        Self {
            // ... existing ...
            your_resources: your_resources_cache,
        }
    }

    pub fn your_resources(&self) -> &ClientCache<YourResource> {
        &self.your_resources
    }

    pub fn list_your_resources(&self) -> ListData<YourResource> {
        self.your_resources.list_owned()
    }

    pub fn is_ready(&self) -> Result<(), String> {
        let mut not_ready = Vec::new();
        // ... existing checks ...
        if !self.your_resources.is_ready() {
            not_ready.push("your_resources");
        }
        // ...
    }

    pub fn list(&self, key: &String, kind: &ResourceKind) -> Result<ListDataSimple, String> {
        let (data_json, resource_version) = match kind {
            // ... existing cases ...
            ResourceKind::YourResource => {
                let list_data = self.your_resources.list();
                let json = serde_json::to_string(&list_data.data)
                    .map_err(|e| format!("Failed to serialize YourResource data: {}", e))?;
                (json, list_data.resource_version)
            }
            // ...
        };
        // ...
    }
}

impl ConfigClientEventDispatcher for ConfigClient {
    fn apply_resource_change(
        &self,
        change: ResourceChange,
        resource_type: Option<ResourceKind>,
        data: String,
        _resource_version: Option<u64>,
    ) {
        match resource_type {
            // ... existing cases ...
            ResourceKind::YourResource => match serde_yaml::from_str::<YourResource>(&data) {
                Ok(resource) => {
                    Self::apply_change_to_cache(&self.your_resources, change, resource);
                }
                Err(e) => log_error("YourResource", &e),
            },
            // ...
        }
    }
}
```

### Step 8: Create Example YAML

Create `config/examples/test_your-resource_YourResource.yaml`:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: YourResource
metadata:
  name: example-resource
  namespace: default
spec:
  # Your spec fields
```

### Step 9: Test

1. Start the operator: `cargo run --bin edgion-op`
2. Start the gateway: `cargo run --bin edgion-gw`
3. Apply your resource: The operator should detect and sync it
4. Verify in logs that the resource is processed

## Cache Synchronization

### Version Management

Resources use Kubernetes' `resourceVersion` for optimistic concurrency control:

1. Server tracks the highest version seen
2. Clients store local versions
3. On reconnect, clients request updates from their last seen version
4. Server streams only newer versions

### Ready State

Caches have a "ready" state that indicates initial synchronization is complete:

1. Cache starts in "not ready" state
2. Server calls `set_ready()` after initial load
3. Clients check `is_ready()` before serving traffic
4. Gateway waits for all caches to be ready before accepting requests

### Event Ordering

Events are processed in order:
1. Server maintains event order per resource type
2. Watch streams guarantee ordered delivery
3. Clients apply changes sequentially

## Best Practices

### 1. Runtime-Only Fields

Use `#[serde(skip)]` and `#[schemars(skip)]` for fields that are computed at runtime:

```rust
#[serde(skip)]
#[schemars(skip)]
pub backend_finder: BackendSelector<GRPCBackendRef>,
```

### 2. Pre-Parsing

Implement `pre_parse()` to populate runtime fields after deserialization:

```rust
fn pre_parse(&mut self) {
    // Parse extension refs
    self.preparse();
    
    // Parse timeouts from strings to Duration
    self.parse_timeouts();
}
```

### 3. Validation

Validate resources in server event dispatch before storing:

```rust
// Check parent references exist
if !gateway_exists {
    tracing::warn!("GRPCRoute references non-existent Gateway, skipping");
    return;
}
```

### 4. Error Handling

Always handle deserialization errors gracefully:

```rust
match serde_yaml::from_str::<GRPCRoute>(&data) {
    Ok(resource) => { /* process */ }
    Err(e) => {
        tracing::error!("Failed to parse GRPCRoute: {}", e);
        return;
    }
}
```

### 5. Logging

Use structured logging with context:

```rust
tracing::info!(
    component = "config_server",
    change = ?change,
    kind = "GRPCRoute",
    route_name = ?resource.metadata.name,
    "Applying GRPCRoute resource change"
);
```

## Examples

### HTTPRoute

HTTPRoute is used for routing HTTP/HTTPS traffic based on:
- Hostname matching
- Path matching (Exact, PathPrefix, RegularExpression)
- Header matching
- Query parameter matching
- HTTP method matching

See `src/types/resources/http_route.rs` for complete implementation.

### GRPCRoute

GRPCRoute is used for routing gRPC traffic based on:
- Hostname matching
- gRPC service and method matching
- Header matching (HTTP/2 headers)

Key differences from HTTPRoute:
- Uses `GRPCMethodMatch` instead of `HTTPPathMatch`
- Method matching format: `service.name/method.name`
- No URL rewrite/redirect filters (not applicable to gRPC)

See `src/types/resources/grpc_route.rs` for complete implementation.

## Troubleshooting

### Resource Not Appearing

1. Check operator logs for deserialization errors
2. Verify the resource passes validation
3. Ensure parent references exist (for routes)
4. Check that the operator is watching the correct namespace

### Cache Not Ready

1. Verify server is running and accessible
2. Check gRPC connectivity
3. Review watch stream for errors
4. Ensure initial list completed successfully

### Updates Not Propagating

1. Verify resource version is incrementing
2. Check watch stream is active
3. Review event dispatch logs
4. Ensure no deserialization errors

## Related Documentation

- [Kubernetes Gateway API Specification](https://gateway-api.sigs.k8s.io/)
- [HTTPRoute Reference](../../../src/types/resources/http_route.rs)
- [GRPCRoute Reference](../../../src/types/resources/grpc_route.rs)
- [负载均衡算法使用](../user-guide/http-route/lb-algorithms.md)

## Conclusion

This architecture provides a robust, type-safe way to sync Kubernetes resources from the API server to the gateway runtime. The pattern is consistent across all resource types, making it easy to add new resources while maintaining reliability and performance.

Key benefits:
- **Type Safety**: Compile-time guarantees through Rust's type system
- **Extensibility**: Easy to add new resource types
- **Efficiency**: Incremental updates via watch streams
- **Reliability**: Version tracking and validation at every step
- **Observability**: Structured logging throughout the pipeline
