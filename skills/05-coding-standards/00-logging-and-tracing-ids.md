# 日志 ID 传播规范

> 目标：任何一个请求失败，都能通过日志同时过滤出 **控制面** 和 **数据面** 的相关记录。

## 核心 ID 三元组

| ID | 含义 | 来源 | 控制面 | 数据面 |
|----|------|------|--------|--------|
| `key_name` | 资源唯一标识 `namespace/name` | `ResourceMeta::key_name()` | ✅ 所有日志 | ✅ access log (`rns`/`rn` 或 `ns`/`name`) |
| `rv` (resource_version) | K8s 资源版本号 | `ResourceMeta::get_version()` / `metadata.resource_version` | ✅ 关键日志 | ❌ 不需要 |
| `sv` (sync_version) | gRPC 同步版本号 | `edgion.io/sync-version` annotation | ✅ gRPC 发送时 | ✅ access log / match info |

## 架构：sv 的生命周期

```
Controller                         Gateway (Data Plane)
┌──────────────────┐               ┌──────────────────────────┐
│ ResourceProcessor│               │ event_dispatch.rs        │
│   process_resource()             │   list_and_reset()       │
│   log: kind/name/rv              │     ↓ set_sync_version() │
│         ↓                        │   watch loop             │
│ ServerCache.apply_change()       │     ↓ set_sync_version() │
│   assign new sync_version        │         ↓                │
│         ↓                        │ ConfHandler.full_set()   │
│ gRPC WatcherEvent                │   / partial_update()     │
│   {type, sync_version, data}  ──►│         ↓                │
│                                  │ RouteManager / TlsStore  │
│                                  │   get_sync_version()     │
│                                  │         ↓                │
│                                  │ MatchInfo.sv / MatchedInfo.sv
│                                  │         ↓                │
│                                  │ AccessLog (sv field)     │
└──────────────────┘               └──────────────────────────┘
```

## sv 存储方式

**使用 `metadata.annotations["edgion.io/sync-version"]`**：

- 所有资源（含 k8s-openapi 的 Service / Endpoints / EndpointSlice）统一适用
- 无需修改 Spec struct
- 注入点在 `event_dispatch.rs`（List 和 Watch 两条路径）
- 通过 `ResourceMeta::get_sync_version()` 读取（宏自动生成）

```rust
// 注入（event_dispatch.rs）
crate::types::resource::meta::set_sync_version(resource.meta_mut(), sv);

// 读取
let sv = resource.get_sync_version(); // 来自 ResourceMeta trait
```

## 控制面日志规范

`processor.rs` 的 `process_resource()` 是所有资源处理的统一入口，关键日志 **必须** 包含 `rv`：

```rust
tracing::warn!(
    kind = self.kind,
    name = %name,
    namespace = %namespace,
    rv,                          // ← 必须
    error = %error,
    "Resource validation error"
);
```

各 Handler 的 `parse()` / `on_change()` 中如需日志，应包含 `kind` + `key_name` + `rv`。

## 数据面日志规范

数据面不使用 `tracing::*` 做请求级日志（见 [01-log-safety.md](01-log-safety.md)），通过 access log 承载。

### HTTP / gRPC 路由

`MatchInfo` 结构自动携带 `sv`，在路由编译时从 `route.get_sync_version()` 注入：

```rust
MatchInfo::new(ns, name, rule_id, match_id, match_item, route_sv)
```

### TLS / TCP / Stream 路由

`MatchedInfo` 结构携带 `sv`：

```rust
MatchedInfo { kind, ns, name, section, sv: resource.get_sync_version() }
```

### 后端 MatchedInfo

后端 MatchedInfo（在 upstream 里）的 `sv` 设为 0——后端资源（Service）有自己的 sv，但与路由匹配无关。

## 跨资源 sv 追踪

**问题**：Secret 不同步到数据面，EdgionTls 依赖 Secret，怎么追踪？

**答案**：通过依赖 requeue 机制自然解决：

1. Secret 变更 → `SecretRefManager` → requeue EdgionTls
2. EdgionTls 重新 `parse()`，拉取最新 Secret，填入 `spec.secret`
3. `ServerCache.apply_change()` 分配 **新的 sync_version**
4. 数据面收到更新后的 EdgionTls，带有新 sv

同理适用于所有依赖关系：

| 依赖方 | 被依赖方 | requeue 机制 | sv 更新 |
|--------|---------|-------------|---------|
| EdgionTls | Secret | SecretRefManager | EdgionTls 获得新 sv |
| Gateway | Secret | SecretRefManager | Gateway 获得新 sv |
| HTTPRoute | Service | ServiceRefManager | HTTPRoute 获得新 sv |
| EdgionPlugins | Secret | SecretRefManager | EdgionPlugins 获得新 sv |
| EdgionPlugins | ConfigMap | ConfigMapRefManager | EdgionPlugins 获得新 sv |
| TLSRoute | Gateway | GatewayRouteIndex | TLSRoute 获得新 sv |

## 排障示例

```bash
# 1. 从 access log 找到 sv
# access.log: {"match_info":{"rns":"prod","rn":"api-route","sv":42},...}

# 2. 用 sv 过滤数据面 conf_sync 日志
grep 'sv.*42' gateway.log

# 3. 用 rv 过滤控制面日志（rv 从 access log 的 resource_version 或 controller log 获取）
grep 'rv.*12345' controller.log

# 4. 如果是 Secret 变更导致的问题（EdgionTls 证书更新）
# 找到 EdgionTls 的新 sv，向上追溯 controller 日志中 Secret requeue 和 EdgionTls reprocess
```

## Key Files

| 文件 | 作用 |
|------|------|
| `src/types/constants/annotations.rs` | `edgion::SYNC_VERSION` 常量定义 |
| `src/types/resource/meta/traits.rs` | `extract_sync_version()` / `set_sync_version()` / `ResourceMeta::get_sync_version()` |
| `src/core/gateway/conf_sync/cache_client/event_dispatch.rs` | sv 注入点（List + Watch） |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs` | rv 日志统一入口 |
| `src/types/ctx.rs` | `MatchInfo.sv` / `MatchedInfo.sv` 定义 |
| `src/core/gateway/routes/grpc/match_unit.rs` | `GrpcMatchInfo.sv` 定义 |
