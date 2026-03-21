---
name: resource-edgion-tls
description: EdgionTls 资源：扩展 TLS 证书配置、mTLS、Secret 引用解析、Gateway 绑定。
---

# EdgionTls 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

EdgionTls 是 Edgion 的自定义扩展资源，提供比 Gateway 内置 certificateRefs 更丰富的 TLS 证书管理能力，支持独立的服务端证书配置、客户端认证（mTLS）、最低 TLS 版本和密码套件选择等。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls.rs`
- 类型定义: `src/types/resources/edgion_tls.rs`

## Controller 侧处理

### validate

1. 检查 `parent_refs` 是否存在且非空，未设置则警告不会生效
2. 从 GLOBAL_SECRET_STORE 检查 `secret_ref` 引用的 Secret 是否存在，不存在则警告（可能后续到达）

### parse

1. **清除旧引用**：清除 SecretRefManager 中该 EdgionTls 的旧记录
2. **解析服务端证书 Secret**：从 `spec.secret_ref` 获取 Secret 引用，在 SecretRefManager 注册，从 GLOBAL_SECRET_STORE 读取 Secret 数据填入 `spec.secret`
3. **解析 CA 证书 Secret（mTLS）**：若配置了 `spec.client_auth.ca_secret_ref`，同样注册并解析 CA Secret，填入 `client_auth.ca_secret`
4. **解析 resolved_ports**：从 parentRef 解析 Listener 端口，逻辑与路由资源一致：parentRef.port 优先，否则通过 sectionName 或全部 Listener 获取，受 allowedRoutes 命名空间策略约束

### on_change

注册到 GatewayRouteIndex，确保 Gateway 的 hostname/port 变更时能 requeue 此 EdgionTls（重新计算 resolved_ports）。

### on_delete

清除 SecretRefManager 引用和 GatewayRouteIndex 注册。

### update_status

为每个 parentRef 生成独立的 RouteParentStatus：
- `Accepted`：无 validation_errors 时为 True，有错误时为 False（reason=Invalid），同时校验 parentRef 对应的 Gateway 是否存在
- `ResolvedRefs`：检查 secret_ref 引用的 Secret 是否在 GLOBAL_SECRET_STORE 中存在，不存在则 False（reason=SecretNotFound）
- 仅设置 Accepted 和 ResolvedRefs 两个 Condition，不设置 Programmed/Ready

## Gateway 侧处理

EdgionTls 同步到 Gateway 后存入 TlsStore，提供 SNI 匹配的证书。请求到达时，根据 TLS ClientHello 的 SNI 字段匹配证书，支持按 hostname 和端口维度的精确匹配。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| EdgionTls → Secret | Secret | secret_ref | 服务端 TLS 证书和私钥 |
| EdgionTls → Secret | Secret | client_auth.ca_secret_ref | mTLS 的 CA 证书 |
| EdgionTls → Gateway | Gateway | parentRef | 绑定到 Gateway 的特定 Listener |
| EdgionTls ← SecretRefManager | Secret | 级联 requeue | Secret 变更时自动重新处理 EdgionTls |
