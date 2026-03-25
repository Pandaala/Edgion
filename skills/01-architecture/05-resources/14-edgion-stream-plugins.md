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

### parse

无特殊处理逻辑，直接透传。EdgionStreamPlugins 的配置在类型层面由预解析处理。

### update_status

- `Accepted`：无 validation_errors 时为 True（reason=Accepted），有错误时为 False（reason=Invalid）
- 使用 k8s_openapi 原生的 `Condition` 类型（与其他资源使用自定义 Condition 类型不同）

## Gateway 侧处理

EdgionStreamPlugins 同步到 Gateway 后，在连接建立阶段执行。典型的 Stream 插件包括：

- **IP 限制**：基于客户端 IP 地址的黑白名单控制
- **TLS 路由选择**：基于 TLS ClientHello 中的 SNI 信息选择路由

这些插件在 TCP 连接级别执行，不需要 HTTP 协议解析。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| EdgionStreamPlugins ← 路由资源 | HTTPRoute/GRPCRoute/TCPRoute/TLSRoute | 关联引用 | 路由资源可引用 Stream 插件 |
