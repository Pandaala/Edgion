# Resource Registry Guide

## Overview

The resource registry (`resource_registry`) is a centralized global registry for managing all resource type metadata. It provides a Single Source of Truth for defining all resource types and their attributes in the system.

## Location

- **Module path**: `src/types/resource_registry.rs`
- **Export**: Via the `crate::types` module

## Core Concepts

### ResourceTypeMetadata

Each resource type has the following metadata:

```rust
pub struct ResourceTypeMetadata {
    /// Resource type name (for display and logging)
    pub name: &'static str,
    /// Resource type description (optional)
    pub description: Option<&'static str>,
    /// Whether it is a base configuration resource
    pub is_base_conf: bool,
}
```

### Global Registry

`RESOURCE_TYPES` is a global static variable containing all registered resource types:

```rust
pub static RESOURCE_TYPES: LazyLock<Vec<ResourceTypeMetadata>> = LazyLock::new(|| {
    vec![
        // Base configuration resources
        ResourceTypeMetadata::new("gateway_classes")
            .with_description("GatewayClass resources")
            .base_conf(),
        ResourceTypeMetadata::new("gateways")
            .with_description("Gateway resources")
            .base_conf(),
        // ... more resources
    ]
});
```

## Usage

### 1. Get All Resource Type Names

```rust
use crate::types::all_resource_type_names;

let resource_names = all_resource_type_names();
// Returns: ["gateway_classes", "gateways", "routes", ...]
```

### 2. Get Base Configuration Resources

```rust
use crate::types::base_conf_resource_names;

let base_conf_resources = base_conf_resource_names();
// Returns: ["gateway_classes", "gateways", "edgion_gateway_configs"]
```

### 3. Query Metadata for a Specific Resource

```rust
use crate::types::get_resource_metadata;

if let Some(metadata) = get_resource_metadata("gateway_classes") {
    println!("Name: {}", metadata.name);
    println!("Is base conf: {}", metadata.is_base_conf);
    if let Some(desc) = metadata.description {
        println!("Description: {}", desc);
    }
}
```

### 4. Iterate Over All Resources

```rust
use crate::types::RESOURCE_TYPES;

for resource in RESOURCE_TYPES.iter() {
    println!("{}: {}", 
        resource.name, 
        resource.description.unwrap_or("No description")
    );
}
```

## Practical Usage Examples

### Usage in ConfigClient

In the `ConfigClient::is_ready()` method, the global registry is used to dynamically check the ready status of all resources:

```rust
fn all_caches_status(&self) -> Vec<(&'static str, bool)> {
    all_resource_type_names()
        .into_iter()
        .filter_map(|name| {
            self.get_cache_status(name).map(|ready| (name, ready))
        })
        .collect()
}

pub fn is_ready(&self) -> Result<(), String> {
    let not_ready: Vec<&str> = self.all_caches_status()
        .into_iter()
        .filter_map(|(name, ready)| if !ready { Some(name) } else { None })
        .collect();
    
    if not_ready.is_empty() {
        Ok(())
    } else {
        Err(format!("wait [{}] ready", not_ready.join(", ")))
    }
}
```

## Advantages

### 1. Centralized Management
All resource type definitions are in one place, making them easy to maintain and extend.

### 2. Type Safety
Uses `&'static str` to ensure resource names are compile-time constants.

### 3. Extensibility
When adding a new resource type:
1. Add one line to `RESOURCE_TYPES`
2. Add the corresponding match branch where the resource is used (e.g., `ConfigClient::get_cache_status`)
3. All features depending on the global registry automatically include the new resource

### 4. Metadata Support
Additional metadata can be easily added to resource types:
- Description information
- Classification markers (e.g., `is_base_conf`)
- Future additions: priority, dependency relationships, etc.

### 5. Consistency
Ensures all parts of the system have a consistent understanding of resource types.

## Adding a New Resource Type

### Step 1: Add Resource to the Registry

Edit `src/types/resource_registry.rs`:

```rust
pub static RESOURCE_TYPES: LazyLock<Vec<ResourceTypeMetadata>> = LazyLock::new(|| {
    vec![
        // ... existing resources
        
        // New resource
        ResourceTypeMetadata::new("my_new_resource")
            .with_description("My new resource type"),
    ]
});
```

### Step 2: Add Cache in ConfigClient

Edit `src/core/conf_sync/conf_client/config_client.rs`:

1. Add field:
```rust
pub struct ConfigClient {
    // ... existing fields
    my_new_resources: ClientCache<MyNewResource>,
}
```

2. Initialize in `new()`:
```rust
let my_new_resources_cache = ClientCache::new(...);
```

3. Add to `get_cache_status()`:
```rust
fn get_cache_status(&self, name: &str) -> Option<bool> {
    match name {
        // ... existing branches
        "my_new_resource" => Some(self.my_new_resources.is_ready()),
        _ => None,
    }
}
```

### Step 3: Add Access Method

```rust
pub fn my_new_resources(&self) -> &ClientCache<MyNewResource> {
    &self.my_new_resources
}
```

## Future Extensions

The global registry provides a foundation for future features:

1. **Dependency management**: Define dependencies between resources
2. **Priority**: Control resource loading order
3. **Resource groups**: Group related resources (e.g., all Route types)
4. **Validation rules**: Define validation rules for resource types
5. **Access control**: Define access permissions for each resource type

## Related Files

- `src/types/resource_registry.rs` - Registry implementation
- `src/types/mod.rs` - Module exports
- `src/core/conf_sync/conf_client/config_client.rs` - Primary consumer
