# Resource Registry Guide

## 概述

资源注册表 (`resource_registry`) 是一个集中管理所有资源类型元数据的全局注册表。它提供了一个单一的真实来源（Single Source of Truth），用于定义系统中所有资源类型及其属性。

## 位置

- **模块路径**: `src/types/resource_registry.rs`
- **导出**: 通过 `crate::types` 模块导出

## 核心概念

### ResourceTypeMetadata

每个资源类型都有以下元数据：

```rust
pub struct ResourceTypeMetadata {
    /// 资源类型名称（用于显示和日志）
    pub name: &'static str,
    /// 资源类型描述（可选）
    pub description: Option<&'static str>,
    /// 是否为基础配置资源
    pub is_base_conf: bool,
}
```

### 全局注册表

`RESOURCE_TYPES` 是一个全局静态变量，包含所有已注册的资源类型：

```rust
pub static RESOURCE_TYPES: LazyLock<Vec<ResourceTypeMetadata>> = LazyLock::new(|| {
    vec![
        // 基础配置资源
        ResourceTypeMetadata::new("gateway_classes")
            .with_description("GatewayClass resources")
            .base_conf(),
        ResourceTypeMetadata::new("gateways")
            .with_description("Gateway resources")
            .base_conf(),
        // ... 更多资源
    ]
});
```

## 使用方式

### 1. 获取所有资源类型名称

```rust
use crate::types::all_resource_type_names;

let resource_names = all_resource_type_names();
// 返回: ["gateway_classes", "gateways", "routes", ...]
```

### 2. 获取基础配置资源

```rust
use crate::types::base_conf_resource_names;

let base_conf_resources = base_conf_resource_names();
// 返回: ["gateway_classes", "gateways", "edgion_gateway_configs"]
```

### 3. 查询特定资源的元数据

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

### 4. 遍历所有资源

```rust
use crate::types::RESOURCE_TYPES;

for resource in RESOURCE_TYPES.iter() {
    println!("{}: {}", 
        resource.name, 
        resource.description.unwrap_or("No description")
    );
}
```

## 实际应用示例

### ConfigClient 中的使用

在 `ConfigClient::is_ready()` 方法中，我们使用全局注册表来动态检查所有资源的就绪状态：

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

## 优势

### 1. 集中管理
所有资源类型的定义都在一个地方，便于维护和扩展。

### 2. 类型安全
使用 `&'static str` 确保资源名称是编译时常量。

### 3. 可扩展性
添加新资源类型时：
1. 在 `RESOURCE_TYPES` 中添加一行
2. 在使用该资源的地方（如 `ConfigClient::get_cache_status`）添加对应的 match 分支
3. 所有依赖全局注册表的功能自动包含新资源

### 4. 元数据支持
可以轻松为资源类型添加额外的元数据：
- 描述信息
- 分类标记（如 `is_base_conf`）
- 未来可以添加更多属性（优先级、依赖关系等）

### 5. 一致性
确保系统各部分对资源类型的理解保持一致。

## 添加新资源类型

### 步骤 1: 在注册表中添加资源

编辑 `src/types/resource_registry.rs`:

```rust
pub static RESOURCE_TYPES: LazyLock<Vec<ResourceTypeMetadata>> = LazyLock::new(|| {
    vec![
        // ... 现有资源
        
        // 新资源
        ResourceTypeMetadata::new("my_new_resource")
            .with_description("My new resource type"),
    ]
});
```

### 步骤 2: 在 ConfigClient 中添加 cache

编辑 `src/core/conf_sync/conf_client/config_client.rs`:

1. 添加字段：
```rust
pub struct ConfigClient {
    // ... 现有字段
    my_new_resources: ClientCache<MyNewResource>,
}
```

2. 在 `new()` 中初始化：
```rust
let my_new_resources_cache = ClientCache::new(...);
```

3. 在 `get_cache_status()` 中添加：
```rust
fn get_cache_status(&self, name: &str) -> Option<bool> {
    match name {
        // ... 现有分支
        "my_new_resource" => Some(self.my_new_resources.is_ready()),
        _ => None,
    }
}
```

### 步骤 3: 添加访问方法

```rust
pub fn my_new_resources(&self) -> &ClientCache<MyNewResource> {
    &self.my_new_resources
}
```

## 未来扩展

全局注册表为未来的功能提供了基础：

1. **依赖关系管理**: 定义资源之间的依赖关系
2. **优先级**: 控制资源的加载顺序
3. **资源组**: 将相关资源分组（如所有 Route 类型）
4. **验证规则**: 定义资源的验证规则
5. **权限控制**: 为每种资源类型定义访问权限

## 相关文件

- `src/types/resource_registry.rs` - 注册表实现
- `src/types/mod.rs` - 模块导出
- `src/core/conf_sync/conf_client/config_client.rs` - 主要使用者

