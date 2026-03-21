---
name: resource-edgion-gateway-config
description: EdgionGatewayConfig 资源：Edgion 扩展的全局网关配置。
---

# EdgionGatewayConfig 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

EdgionGatewayConfig 是集群作用域（cluster-scoped）的 Edgion 扩展资源，用于配置全局网关行为参数，属于 Gateway API 标准之外的自定义扩展。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs`
- Gateway ConfHandler: `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`
- 类型定义: `src/types/resources/edgion_gateway_config.rs`

## Controller 侧处理

### parse

无特殊处理逻辑，直接透传。EdgionGatewayConfig 的 spec 内容在预解析阶段由类型自身处理。

### update_status

- `Accepted`：无 validation_errors 时为 True（reason=Accepted），有错误时为 False（reason=Invalid）

## Gateway 侧处理

EdgionGatewayConfig 同步到 Gateway 后，其配置项应用于所有 Gateway 实例的全局行为（如默认超时、缓冲区大小等全局参数）。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| EdgionGatewayConfig → Gateway | Gateway | 全局 | 全局配置影响所有 Gateway 的行为 |
