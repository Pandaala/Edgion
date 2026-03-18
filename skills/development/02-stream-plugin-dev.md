---
name: stream-plugin-development
description: Use when implementing or extending EdgionStreamPlugins, wiring Gateway or route-level stream-plugin annotations, or debugging the ConnectionFilter and TLSRoute plugin runtimes.
---
# Stream Plugin Development

用于回答这些问题：

- 怎么新增或扩展 `EdgionStreamPlugins`
- Gateway / `TCPRoute` / `TLSRoute` 上的 `edgion.io/edgion-stream-plugins` 到底接到哪里
- Stage 1 `ConnectionFilter` 和 Stage 2 `TlsRoute` 插件运行时的区别是什么
- 为什么 YAML 改了，但 Gateway 上没有热更新或运行结果不对

## 先看这些真实入口

- [../../src/types/resources/edgion_stream_plugins/mod.rs](../../src/types/resources/edgion_stream_plugins/mod.rs)
- [../../src/types/resources/edgion_stream_plugins/stream_plugins.rs](../../src/types/resources/edgion_stream_plugins/stream_plugins.rs)
- [../../src/types/resources/edgion_stream_plugins/tls_route_plugins.rs](../../src/types/resources/edgion_stream_plugins/tls_route_plugins.rs)
- [../../src/types/resource/meta/impls.rs](../../src/types/resource/meta/impls.rs)
- [../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs](../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs)
- [../../src/core/gateway/plugins/stream/mod.rs](../../src/core/gateway/plugins/stream/mod.rs)
- [../../src/core/gateway/plugins/stream/stream_plugin_trait.rs](../../src/core/gateway/plugins/stream/stream_plugin_trait.rs)
- [../../src/core/gateway/plugins/stream/stream_plugin_runtime.rs](../../src/core/gateway/plugins/stream/stream_plugin_runtime.rs)
- [../../src/core/gateway/plugins/stream/stream_plugin_store.rs](../../src/core/gateway/plugins/stream/stream_plugin_store.rs)
- [../../src/core/gateway/runtime/server/listener_builder.rs](../../src/core/gateway/runtime/server/listener_builder.rs)
- [../../src/core/gateway/routes/tcp/conf_handler_impl.rs](../../src/core/gateway/routes/tcp/conf_handler_impl.rs)
- [../../src/core/gateway/routes/tls/proxy.rs](../../src/core/gateway/routes/tls/proxy.rs)

## 心智模型

这套链路不是单纯的“写一个 trait”。
当前仓库里的真实路径是：

1. 定义 `EdgionStreamPlugins` 资源结构
2. 在 `ResourceMeta` 预处理里初始化运行时对象
3. Controller 负责接收资源、校验并写 status
4. Gateway `ConfigClient` 把资源同步进 `StreamPluginStore`
5. Gateway / `TCPRoute` / `TLSRoute` 通过 `edgion.io/edgion-stream-plugins` 注解引用资源
6. 数据面在不同阶段执行运行时：
   - Stage 1: `ConnectionFilter`，握手前，只有 IP / 端口等连接上下文
   - Stage 2: `TlsRoute`，TLS 握手与路由匹配后，可以拿到 SNI、匹配路由、mTLS 状态等

## 当前代码布局

```text
src/types/resources/edgion_stream_plugins/
  mod.rs                    # CRD、spec/status、runtime 初始化入口
  stream_plugins.rs         # Stage 1 plugin enum: EdgionStreamPlugin
  tls_route_plugins.rs      # Stage 2 plugin enum: TlsRouteStreamPlugin

src/core/gateway/plugins/stream/
  stream_plugin_trait.rs    # StreamPlugin / StreamContext / StreamPluginResult
  stream_plugin_runtime.rs  # Stage 1 runtime: 从 entries 构建插件链
  stream_plugin_store.rs    # Gateway 全局热更新 store（ArcSwap）
  connection_filter_bridge.rs
  ip_restriction/           # 现成参考实现
  tls_route/                # Stage 2 runtime 和 trait
```

## 两个运行阶段

### Stage 1: ConnectionFilter

- trait 在 [../../src/core/gateway/plugins/stream/stream_plugin_trait.rs](../../src/core/gateway/plugins/stream/stream_plugin_trait.rs)
- 上下文是 `StreamContext`
- 返回值是：
  - `StreamPluginResult::Allow`
  - `StreamPluginResult::Deny(String)`
- 典型绑定入口：
  - Gateway listener 级别：`listener_builder.rs`
  - `TCPRoute` 级别：`routes/tcp/conf_handler_impl.rs`
- 适合：
  - IP allow/deny
  - 尽早拒绝连接
  - 不需要 SNI / route match / mTLS 信息的逻辑

### Stage 2: TlsRoute

- 运行时入口在 [../../src/core/gateway/routes/tls/proxy.rs](../../src/core/gateway/routes/tls/proxy.rs)
- 优先执行 `tls_route_plugin_runtime`
- 如果资源没有 `tlsRoutePlugins`，但有老的 `plugins`，当前实现会 fallback 到 Stage 1 runtime 做兼容
- 适合：
  - 依赖 SNI 的逻辑
  - 依赖匹配到哪条 `TLSRoute` 的逻辑
  - 依赖 `is_mtls` 等 TLS 上下文的逻辑

## 新增一个 stream plugin 的最小清单

### 1. 先决定它属于哪个阶段

- 只依赖 IP / 连接端口：放 Stage 1
- 依赖 SNI / 路由 / mTLS：放 Stage 2
- 两边都要：可以同时在 `EdgionStreamPlugin` 和 `TlsRouteStreamPlugin` 里接入

### 2. 定义或复用配置结构

当前 `IpRestriction` 直接复用了 HTTP 插件侧的 `IpRestrictionConfig`：

- [../../src/types/resources/edgion_plugins/plugin_configs/](../../src/types/resources/edgion_plugins/plugin_configs/)

如果你的 stream plugin 也能和 HTTP 层共享配置，优先复用；否则再新增专属配置。

## 3. 把新类型接进资源 enum

Stage 1:

- 修改 [../../src/types/resources/edgion_stream_plugins/stream_plugins.rs](../../src/types/resources/edgion_stream_plugins/stream_plugins.rs)

Stage 2:

- 修改 [../../src/types/resources/edgion_stream_plugins/tls_route_plugins.rs](../../src/types/resources/edgion_stream_plugins/tls_route_plugins.rs)

至少要补：

- enum variant
- `type_name()`
- serde / schema 兼容性

## 4. 实现运行时插件

Stage 1 插件要实现：

```rust
#[async_trait]
pub trait StreamPlugin: Send + Sync {
    fn name(&self) -> &str;
    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult;
}
```

最直接的参考实现：

- [../../src/core/gateway/plugins/stream/ip_restriction/stream_ip_restriction.rs](../../src/core/gateway/plugins/stream/ip_restriction/stream_ip_restriction.rs)

Stage 2 插件则看：

- [../../src/core/gateway/plugins/stream/tls_route/](../../src/core/gateway/plugins/stream/tls_route/)

## 5. 在 runtime 里注册构造逻辑

Stage 1 runtime：

- [../../src/core/gateway/plugins/stream/stream_plugin_runtime.rs](../../src/core/gateway/plugins/stream/stream_plugin_runtime.rs)

Stage 2 runtime：

- [../../src/core/gateway/plugins/stream/tls_route/tls_route_plugin_runtime.rs](../../src/core/gateway/plugins/stream/tls_route/tls_route_plugin_runtime.rs)

这里是“资源配置 -> 运行时插件对象”的真正装配点。只改 enum、不改 runtime，Gateway 不会执行你的新插件。

## 6. 确认预处理和热更新链路不用补洞

当前 `EdgionStreamPlugins` 的 runtime 初始化不是在 handler 里做，而是在：

- [../../src/types/resource/meta/impls.rs](../../src/types/resource/meta/impls.rs)

也就是 `ResourceMeta` 预处理阶段会调用：

- `init_stream_plugin_runtime()`
- `init_tls_route_plugin_runtime()`

Controller handler [../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs](../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs) 目前主要负责 status，而不是构建运行时。

Gateway 侧热更新入口是：

- [../../src/core/gateway/plugins/stream/stream_plugin_store.rs](../../src/core/gateway/plugins/stream/stream_plugin_store.rs)
- [../../src/core/gateway/conf_sync/conf_client/config_client.rs](../../src/core/gateway/conf_sync/conf_client/config_client.rs)

## 7. 绑定 YAML 注解并验证引用路径

当前主键是：

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "namespace/name"
```

现有绑定点：

- Gateway listener 级别：`listener_builder.rs`
- `TCPRoute`：`routes/tcp/conf_handler_impl.rs`
- `TLSRoute`：`routes/tls/conf_handler_impl.rs` + `routes/tls/proxy.rs`

如果只写了资源和 runtime，但没有把路由 / listener 侧引用接上，运行时也不会触发。

我当前没有在仓库里找到 `UDPRoute` 对这个注解的专用处理代码，所以不要默认认为 UDP 已经和 TCP/TLS 一样接通。

## 最小 YAML 例子

### 定义资源

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tcp-allow-localhost
  namespace: edgion-test
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - 127.0.0.1/32
```

### 在 `TCPRoute` / `TLSRoute` / Gateway 上引用

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: edgion-test/tcp-allow-localhost
```

## 和 HTTP 插件的关键区别

| 维度 | Stream plugin | HTTP plugin |
|------|---------------|-------------|
| 执行层级 | TCP/TLS 连接层 | HTTP 请求/响应层 |
| 主要上下文 | `StreamContext` / `TlsRouteContext` | `PluginSession` |
| 失败动作 | 拒绝连接 | 返回 HTTP 响应或终止请求 |
| 绑定方式 | `edgion.io/edgion-stream-plugins` 注解引用资源 | Route/Gateway 上的 `EdgionPlugins` 运行时链 |
| 热更新来源 | `StreamPluginStore` | HTTP plugin runtime/store |

## 测试建议

优先补这三类：

1. 资源状态是否正确
2. Gateway / `TCPRoute` / `TLSRoute` 引用后是否真的命中插件
3. 更新 / 删除 `EdgionStreamPlugins` 后，热更新是否生效、旧配置是否残留

参考现有样例：

- [../../examples/test/conf/Gateway/StreamPlugins/](../../examples/test/conf/Gateway/StreamPlugins/)
- [../../examples/test/conf/TCPRoute/StreamPlugins/](../../examples/test/conf/TCPRoute/StreamPlugins/)
- [../../examples/test/conf/TLSRoute/StreamPlugins/](../../examples/test/conf/TLSRoute/StreamPlugins/)

## 审查清单

- 新插件到底属于 Stage 1、Stage 2，还是两边都要
- enum、runtime、module export 是否都已接线
- 是否复用了已有配置结构，而不是复制一份几乎相同的 config
- `ResourceMeta` 预处理后，runtime 字段是否真的初始化
- 注解解析是否支持 `name` 和 `namespace/name` 两种引用格式
- 更新 / 删除资源后，Gateway store 是否能正确热更新

## 相关

- [05-annotations-reference.md](05-annotations-reference.md)
- [08-conf-handler-guidelines.md](08-conf-handler-guidelines.md)
- [../testing/00-integration-testing.md](../testing/00-integration-testing.md)
