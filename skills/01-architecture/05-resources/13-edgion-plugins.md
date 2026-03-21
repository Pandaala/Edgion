---
name: resource-edgion-plugins
description: EdgionPlugins 资源：HTTP 插件配置、Secret 引用解析、PluginRuntime 构建、ExtensionRef 绑定。
---

# EdgionPlugins 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

EdgionPlugins 是 Edgion 的自定义扩展资源，定义 HTTP 层的插件配置。支持 28 种内置插件（认证、限流、重写、CORS 等），通过 HTTPRoute/GRPCRoute 的 ExtensionRef filter 引用。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`
- Gateway ConfHandler: `src/core/gateway/plugins/http/conf_handler_impl.rs`
- 插件类型定义: `src/types/resources/edgion_plugins/`
- 插件配置类型: `src/types/resources/edgion_plugins/plugin_configs/`

## Controller 侧处理

### parse

EdgionPluginsHandler 的 parse 阶段执行大量 Secret 引用解析，为认证类插件填充凭证数据：

1. **清除旧引用**：清除 SecretRefManager 中该 EdgionPlugins 的旧记录
2. **BasicAuth Secret 解析**：遍历 `secret_refs`，从 Secret 读取 username/password，填入 `resolved_users`
3. **JwtAuth Secret 解析**：遍历 `secret_refs`，从 Secret 读取 JWT 签名密钥（支持 RSA/EC/HMAC/EdDSA 多种算法），解析公钥元数据（KeyMetadata），填入 `resolved_credentials`
4. **JweAuth Secret 解析**：类似 JwtAuth，解析 JWE 加密密钥
5. **HmacAuth Secret 解析**：从 Secret 读取 HMAC 凭证（access_key + secret_key），填入 `resolved_credentials`
6. **KeyAuth Secret 解析**：从 Secret 读取 API key，填入 `resolved_keys`
7. **OIDC Secret 解析**：从 Secret 读取 client_secret 和 session_secret
8. **CertAuth 证书解析**：根据 `cert_source` 模式（Inline/Secret/SecretCa），从 Secret 读取 CA 证书
9. **每个 Secret 引用**都在 SecretRefManager 注册，确保 Secret 变更时自动 requeue

### on_delete

清除 SecretRefManager 中该 EdgionPlugins 的所有引用。

### update_status

- `Accepted`：无 validation_errors 时为 True，有错误时为 False

## Gateway 侧处理

EdgionPlugins 同步到 Gateway 后，存入插件配置存储。HTTPRoute/GRPCRoute 通过 ExtensionRef 引用 EdgionPlugins，在路由 preparse 阶段构建 `PluginRuntime` 执行链。请求处理时按配置的执行阶段（request_plugins/response_plugins）依次执行插件。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| EdgionPlugins → Secret | Secret | secret_refs（多种插件） | 认证插件引用 Secret 中的凭证数据 |
| EdgionPlugins ← HTTPRoute | HTTPRoute | ExtensionRef filter | HTTPRoute 通过 ExtensionRef 引用插件 |
| EdgionPlugins ← GRPCRoute | GRPCRoute | ExtensionRef filter | GRPCRoute 通过 ExtensionRef 引用插件 |
| EdgionPlugins ← SecretRefManager | Secret | 级联 requeue | Secret 变更时自动重新处理 |
