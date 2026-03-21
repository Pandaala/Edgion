---
name: gateway-link-sys
description: LinkSys 外部系统集成：Elasticsearch、Etcd、Redis、Webhook、LocalFile 提供商、数据发送运行时。
---

# LinkSys 外部系统集成

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### 概述

<!-- TODO: 可插拔的外部系统集成，用于日志/数据/配置的外发 -->

### Provider 列表

<!-- TODO:
link_sys/providers/
├── elasticsearch/   # ES 客户端 + Bulk 操作
├── etcd/           # Etcd v3 API 客户端
├── redis/          # Redis 客户端（standalone/sentinel/cluster，基于 fred）
├── webhook/        # HTTP webhook（KeyGet 解析）
├── local_file/     # 文件日志 + 轮转
└── mod.rs          # Provider 工厂
-->

### 运行时

<!-- TODO:
link_sys/runtime/
├── store.rs        # LinkSysStore 管理所有系统客户端
├── data_sender.rs  # 抽象数据发送接口
├── conf_handler.rs # LinkSys 资源更新处理
└── mod.rs          # 运行时协调器
-->

### 与 LinkSys 资源的关系

<!-- TODO: LinkSys CRD 定义连接参数，Gateway 侧 ConfHandler 解析并创建 Provider 实例 -->
