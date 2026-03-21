---
name: resource-gateway-class
description: GatewayClass 资源：集群级别的控制器匹配与过滤机制。
---

# GatewayClass 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

GatewayClass 是集群作用域（cluster-scoped）资源，定义 Gateway 控制器的类型标识。Edgion 使用它实现控制器匹配过滤：只有 `spec.controllerName` 匹配 `edgion.io/gateway-controller` 的 GatewayClass 才会被处理。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class.rs`
- Gateway ConfHandler: `src/core/gateway/config/gateway_class/conf_handler_impl.rs`
- 类型定义: `src/types/resources/gateway_class.rs`

## Controller 侧处理

### filter

按 `spec.controllerName` 过滤，仅处理 controllerName 与配置匹配的 GatewayClass（默认 `edgion.io/gateway-controller`）。

### parse

无特殊处理逻辑，直接透传。

### update_status

- `Accepted`：无 validation_errors 时为 True（reason=Accepted），有错误时为 False（reason=Invalid）
- `SupportedVersion`：始终为 True，表示支持当前 Gateway API 版本

## Gateway 侧处理

GatewayClass 同步到 Gateway 后存入 GatewayConfigStore，用于 Gateway 资源的 class 匹配校验。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| GatewayClass ← Gateway | Gateway | gatewayClassName | Gateway 通过 gatewayClassName 字段引用 GatewayClass |
