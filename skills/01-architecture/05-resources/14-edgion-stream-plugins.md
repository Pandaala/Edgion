---
name: resource-edgion-stream-plugins
description: EdgionStreamPlugins 资源：Stream/TCP 层插件配置、连接级别过滤。
---

# EdgionStreamPlugins 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

EdgionStreamPlugins 是 Edgion 的自定义扩展资源，定义 Stream/TCP 层的插件配置。与 EdgionPlugins 工作在 HTTP 层不同，EdgionStreamPlugins 在连接建立阶段（ConnectionFilter）执行，适用于 IP 限制、TLS 路由选择等场景。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs`
- 类型定义: `src/types/resources/edgion_stream_plugins/`

## Controller 侧处理

### preparse

扫描 Stage 1 `plugins` 数组，检测 legacy HTTP 字段写到 L4 `IpRestriction` 配置上的陈旧 schema，带 `[LEGACY_SCHEMA_DEPRECATED]` 前缀返回 error。update_status 识别前缀后把 reason 映射为 `LegacySchemaDeprecated` 而不是泛化的 `Invalid`，方便 operator 定位迁移指引。

### parse

- **Cross-PM ref 注册**：先 `clear_refs_from` 本资源在 `plugin_metadata_refs` 中的旧 ref，再遍历 Stage 1 每个 `EdgionStreamPlugin::GlobalConnectionIpRestriction` entry，将 `plugin_metadata_ref.namespace/name` 注册为 cluster_pm_key，value 为 `"EdgionStreamPlugins:<this_ns>/<this_name>"`
- 这样当被引用的 GCIR PM 变更时，PM handler 的 `requeue_pm_referrers` 会把 ESP 重新入队，刷新 ResolvedRefs

### on_delete

- `clear_refs_from` 清除本资源在 `plugin_metadata_refs` 中的所有注册

### update_status

使用 `k8s_openapi::Condition` 类型，写两个 condition：

- **`Accepted`**：无 validation_errors 时为 True（reason=Accepted）；有 legacy-schema 错误时为 False/`LegacySchemaDeprecated`；其他错误为 False/`Invalid`
- **`ResolvedRefs`**：遍历 Stage 1 每个 `GlobalConnectionIpRestriction` entry，要求 `plugin_metadata_ref` 指向的 PM 存在且 `spec.metadata.type = GlobalConnectionIpRestriction`。任何不满足 → `ResolvedRefsError::PluginMetaDataNotFound`，reason 为 `InvalidPluginMetaDataRef`。Stage 2 `tls_route_plugins` 目前都是内联配置，无外部 ref

## Gateway 侧处理

EdgionStreamPlugins 同步到 Gateway 后，在连接建立阶段执行。典型的 Stream 插件包括：

- **IP 限制**：基于客户端 IP 地址的黑白名单控制
- **TLS 路由选择**：基于 TLS ClientHello 中的 SNI 信息选择路由

这些插件在 TCP 连接级别执行，不需要 HTTP 协议解析。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| EdgionStreamPlugins ← 路由资源 | HTTPRoute/GRPCRoute/TCPRoute/TLSRoute | 关联引用 | 路由资源可引用 Stream 插件 |
| EdgionStreamPlugins → PluginMetaData | PluginMetaData | `GlobalConnectionIpRestriction.pluginMetadataRef` | 引用 GCIR PM；通过 `plugin_metadata_refs` 建立反向 requeue 索引，PM 变更时 ESP 被重新处理 |
