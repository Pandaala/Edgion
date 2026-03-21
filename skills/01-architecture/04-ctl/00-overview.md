---
name: ctl-overview
description: edgion-ctl 总览：三种 Target 模式（center/server/client）、CLI 参数、EdgionClient。
---

# edgion-ctl 总览

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### 三种 Target 模式

<!-- TODO:
1. center（默认，端口 5800）— ConfCenter API，支持完整 CRUD
2. server（端口 5800）— ConfigServer 缓存（只读）
3. client（端口 5900）— ConfigClient 缓存（只读，Gateway 侧）
-->

### CLI 参数

<!-- TODO:
--target / -t     Target API (center, server, client)
--server          服务器地址
--socket          Unix socket 路径
-f, --file        文件/目录路径（apply/delete 用）
-n, --namespace   命名空间过滤
-o, --output      输出格式 (table, json, yaml, wide)
--dry-run         Dry run 模式
-->

### EdgionClient

<!-- TODO: HTTP 客户端实现，支持 HTTP 和 Unix socket 两种连接方式 -->

### 模块结构

<!-- TODO:
src/core/ctl/
├── cli/
│   ├── mod.rs          # Cli 结构和命令路由
│   ├── client.rs       # EdgionClient 实现
│   ├── output.rs       # 输出格式化
│   └── commands/       # 子命令
└── mod.rs
-->
