---
name: resource-edgion-acme
description: EdgionAcme 资源：ACME 自动证书签发、Let's Encrypt 集成、DNS 提供商、仅 Leader 执行。
---

# EdgionAcme 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- 定义 ACME 自动证书配置
- 域名列表、DNS 提供商配置
- 自动续期
-->

### Controller 侧处理

<!-- TODO:
- EdgionAcmeHandler
- 仅 Leader 执行签发（HTTP-01 挑战需要单点）
- 签发成功后存储到 Secret
-->

### Gateway 侧处理

<!-- TODO: 通过 Secret 间接获取证书 -->

### 跨资源关联

<!-- TODO:
- → Secret: 签发的证书存储
- ← Gateway: 使用签发的证书
-->
