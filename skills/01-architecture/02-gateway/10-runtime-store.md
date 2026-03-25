---
name: gateway-runtime-store
description: 运行时存储：Gateway 资源管理、路由配置存储、ArcSwap 无锁读取、Listener 配置、错误响应生成。
---

# 运行时存储

> 运行时存储模块管理 Gateway 资源配置、路由管理器和监听器信息，
> 使用 ArcSwap 实现无锁读取，支持运行时动态配置更新。

## Gateway Store

`GatewayStore` 存储 Gateway CRD 资源：

```rust
pub struct GatewayStore {
    gateways: HashMap<String, Gateway>,
}
```

操作：
- `add_gateway()` — 添加（key 重复时返回错误）
- `update_gateway()` — 更新或插入
- `remove_gateway()` — 删除
- `get_gateway()` — 按 key 查询
- `list_gateways()` — 列举所有
- `clear()` — 清空

全局单例：`get_global_gateway_store()` 返回 `Arc<RwLock<GatewayStore>>`。

## Gateway 配置存储（GatewayConfigStore）

`GatewayConfigStore` 提供按端口查询的 Gateway 动态配置，采用两层匹配结构：

```rust
pub struct GatewayListenerConfig {
    host_map: Option<HashMap<String, Arc<ListenerConfig>>>,           // 精确主机名匹配
    wildcard_host_map: Option<HashHost<Arc<ListenerConfig>>>,         // 通配符主机名匹配
    listener_map: Option<HashMap<String, Arc<ListenerConfig>>>,       // 按 listener name 匹配
}
```

`ListenerConfig` 包含：
- `name` — Listener 名称
- `port` — 监听端口
- `hostname` — 可选的 SNI 主机名
- `allowed_routes` — 允许的路由配置

路由匹配策略：
- **带 sectionName** 的路由：通过 `listener_map` 精确匹配 listener name
- **不带 sectionName** 的路由：通过 `host_map`（精确）或 `wildcard_host_map`（通配符）匹配主机名

性能优化：所有内部 HashMap 使用 `Option` 类型，大多数 Gateway 不配置 hostname，可跳过 host_map 查询。

全局单例：`get_global_gateway_config_store()`

## PortGatewayInfo Store

`PortGatewayInfoStore` 管理每个端口的 Gateway 信息聚合视图：

- `get_port_gateway_info_store()` — 获取全局实例
- `rebuild_port_gateway_infos()` — 重建端口-Gateway 映射关系
- 提供 `GatewayInfo`（包含 Gateway 的聚合运行时信息）

## ArcSwap 无锁读取模式

运行时存储广泛使用 ArcSwap 模式实现高性能并发访问：

```
写入路径（低频）：                    读取路径（高频）：
ConfHandler.full_set()               store.load()  -> Arc<T>
  -> 构建新 HashMap                     -> 零成本原子读
  -> ArcSwap.store(Arc::new(map))      -> 无锁、无拷贝
```

使用 ArcSwap 的存储：
- `GatewayConfigStore` — Gateway listener 配置
- `LinkSysStore` — LinkSys 资源
- `PluginStore` — EdgionPlugins 资源
- 各 provider 运行时 store（Redis/Etcd/ES）
- `BackendSelector` — 后端选择器状态

与 RwLock 的区别：GatewayStore 使用 `Arc<RwLock<GatewayStore>>`（写操作需要互斥），适用于写频率较高的场景。

## 错误响应生成

Gateway 运行时提供标准化的错误响应生成函数：

| 函数 | 状态码 | 用途 |
|------|--------|------|
| `end_response_400()` | 400 Bad Request | 请求格式错误 |
| `end_response_404()` | 404 Not Found | 路由未匹配 |
| `end_response_421()` | 421 Misdirected Request | SNI 不匹配 |
| `end_response_500()` | 500 Internal Server Error | 内部错误 |
| `end_response_503()` | 503 Service Unavailable | 无可用后端 |

## 目录布局

```
src/core/gateway/runtime/
└── store/
    ├── mod.rs                # 模块导出
    ├── gateway.rs            # GatewayStore（Gateway 资源 CRUD）
    ├── config.rs             # GatewayConfigStore（按端口的 Listener 配置）
    └── port_gateway_info.rs  # PortGatewayInfoStore（端口-Gateway 映射）
```
