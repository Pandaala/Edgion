---
name: resource-gateway
description: Gateway 资源架构：Listener 定义、端口管理、TLS 证书绑定、Route 挂载、Status 管理。
---

# Gateway 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

Gateway 是 Edgion 的核心资源，定义网关的 Listener（监听端口、协议、主机名、TLS 配置），是所有路由资源的挂载点。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs`
- Gateway ConfHandler: `src/core/gateway/config/` (GatewayConfigStore)
- 类型定义: `src/types/resources/gateway.rs`

## Controller 侧处理

### filter

按 `gatewayClassName` 过滤。若 `gateway_class_name` 配置为 `Some(name)`（K8s 模式），仅处理匹配的 Gateway；若为 `None`（FileSystem 模式），处理所有 Gateway。

### parse

1. **清除旧引用**：清除 SecretRefManager 和 CrossNsRefManager 中该 Gateway 的旧记录（处理更新场景）
2. **注册端口信息**：将每个 Listener 的 `(name, port_key)` 注册到 `ListenerPortManager`，用于检测跨 Gateway 的端口冲突。port_key 由 `(port, protocol, hostname)` 组成
3. **解析 TLS 证书**：遍历每个 Listener 的 `tls.certificateRefs`：
   - 在 SecretRefManager 中注册引用关系（Secret 变更时自动 requeue 此 Gateway）
   - 若为跨命名空间引用，在 CrossNsRefManager 中注册
   - 从 `GLOBAL_SECRET_STORE` 读取 Secret 数据，填入 `tls.secrets` 字段
   - Secret 尚未到达时不报错，SecretRefManager 保证后续自动重新处理

### on_change

1. **requeue 冲突 Gateway**：通过 `ListenerPortManager.take_affected_gateways()` 获取冲突关系发生变化的 Gateway 并 requeue，确保双向标记冲突状态
2. **requeue 关联路由**：通过 `GatewayRouteIndex` 查找所有 parentRef 指向此 Gateway 的路由，当 Listener 的 hostname 或 port 发生变化时 requeue 它们，重新计算 resolved_hostnames 和 resolved_ports

### on_delete

- 清除 SecretRefManager、CrossNsRefManager、ListenerPortManager 注册
- requeue 之前与此 Gateway 冲突的其他 Gateway（从 Conflicted=True 变为 False）
- 清除 GatewayRouteIndex 中的 hostname/port 缓存

### update_status

- **Gateway 级别 Conditions**:
  - `Accepted`：无 validation_errors 时为 True
  - `ListenersNotValid`：有端口冲突时为 True
- **Listener 级别**（每个 Listener 独立）：
  - `Accepted`：无 validation_errors 时为 True
  - `Conflicted`：通过 ListenerPortManager 检测，有冲突时为 True
  - `ResolvedRefs`：检查 certificateRef 的 kind（必须为 Secret）、Secret 是否存在、Secret 数据是否合法（包含 tls.crt/tls.key、非空、有效 PEM）、跨命名空间引用是否被 ReferenceGrant 允许
  - `supportedKinds`：基于协议计算支持的路由类型（HTTP/HTTPS → HTTPRoute+GRPCRoute，TCP → TCPRoute，UDP → UDPRoute，TLS → TLSRoute），如果 `allowedRoutes.kinds` 中有不匹配的类型则报 InvalidRouteKind
  - `attachedRoutes`：从 AttachedRouteTracker 获取挂载的路由数量
- **Addresses**：按优先级派生：spec.addresses > 同名 Service 的 ClusterIP > 默认地址配置 > "0.0.0.0"

## Gateway 侧处理

Gateway 资源同步到 Gateway 侧后，用于配置 Pingora 的监听器。每个 Listener 对应一个端口的监听，GatewayConfigStore 按端口维度管理配置。TLS 证书存储在证书匹配引擎中，请求时通过 SNI 匹配选择证书。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| Gateway ← Routes | HTTPRoute/GRPCRoute/TCPRoute/TLSRoute/UDPRoute | parentRef | 路由通过 parentRef 挂载到 Gateway 的 Listener |
| Gateway → Secret | Secret | certificateRefs | Listener TLS 配置引用 Secret 中的证书 |
| Gateway ↔ Gateway | Gateway | ListenerPortManager | 不同 Gateway 的 Listener 占用相同端口时产生冲突 |
| Gateway ← EdgionTls | EdgionTls | parentRef | EdgionTls 通过 parentRef 绑定到 Gateway |
| Gateway ← GatewayClass | GatewayClass | gatewayClassName | Gateway 引用 GatewayClass |
| Gateway ← EdgionGatewayConfig | EdgionGatewayConfig | 全局 | 全局配置影响所有 Gateway |
