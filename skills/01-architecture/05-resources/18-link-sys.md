---
name: resource-link-sys
description: LinkSys 资源：外部系统连接器定义、Provider 配置、拓扑构建。
---

# LinkSys 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- 定义外部系统连接配置
- 支持多种 Provider：Elasticsearch、Etcd、Redis、Webhook、LocalFile
- 连接参数、认证信息
-->

### Controller 侧处理

<!-- TODO:
- LinkSysHandler
- 预解析构建 LinkSys 拓扑
-->

### Gateway 侧处理

<!-- TODO:
- ConfHandler 解析并创建 Provider 实例
- LinkSysStore 管理所有系统客户端
- 供 AccessLog、Plugin 等使用
-->

### 跨资源关联

<!-- TODO:
- → Secret: 连接凭证
- ← AccessLog/Plugin: 数据发送目标
-->
