---
name: resource-edgion-tls
description: EdgionTls 资源：扩展 TLS 证书配置、与 Gateway certificateRefs 的关系、Secret 引用。
---

# EdgionTls 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- Edgion 扩展的 TLS 证书配置
- 可被 Gateway Listener 的 certificateRefs 引用
- 引用 Secret 中的证书/密钥
- 支持额外的 TLS 配置选项（如协议版本、密码套件）
-->

### Controller 侧处理

<!-- TODO: EdgionTlsHandler, Secret 引用校验 -->

### Gateway 侧处理

<!-- TODO: 更新 TLS Store 中的证书 -->

### 跨资源关联

<!-- TODO:
- → Secret: 证书/密钥引用
- ← Gateway: Listener certificateRefs 引用
- → ReferenceGrant: 跨命名空间 Secret 引用
-->
