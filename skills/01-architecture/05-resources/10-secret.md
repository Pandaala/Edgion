---
name: resource-secret
description: Secret 资源：证书/凭证存储、跨资源依赖追踪、不同步到 Gateway、SecretRefManager。
---

# Secret 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- 存储 TLS 证书/密钥、认证凭证等敏感数据
- **不同步到 Gateway**（安全考虑）
- Controller 侧解析后，将解析结果嵌入依赖资源
-->

### Controller 侧处理

<!-- TODO:
- SecretHandler
- GLOBAL_SECRET_STORE: lazy-lock 全局存储
- SecretRefManager: 追踪哪些资源引用了哪些 Secret
- Secret 变更时级联 requeue 所有依赖资源
-->

### Gateway 侧处理

<!-- TODO: Secret 不直接同步到 Gateway，其内容由依赖资源携带 -->

### 跨资源关联

<!-- TODO:
- ← Gateway: TLS certificateRefs
- ← EdgionTls: 证书引用
- ← EdgionPlugins: 认证插件凭证
- ← EdgionAcme: ACME 签发的证书存储
- ← BackendTLSPolicy: 后端 TLS 证书
-->
