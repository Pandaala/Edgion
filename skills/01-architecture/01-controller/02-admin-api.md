---
name: controller-admin-api
description: edgion-controller Admin HTTP API：端点列表、CRUD 操作、健康检查、ConfigServer 接口。
---

# Admin API (:5800)

> **状态**: 框架已建立，待填充详细内容。

## 概要

Controller 通过 Axum 提供 HTTP Admin API，端口 5800，供 edgion-ctl 和运维使用。

## 待填充内容

### 端点列表

<!-- TODO: 完整的 API 端点表 -->
| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/health` | 存活探测 |
| GET | `/ready` | 就绪探测（检查 ConfigSyncServer 是否就绪） |
| GET | `/api/v1/server-info` | 获取 server_id、就绪状态 |
| POST | `/api/v1/reload` | 触发全量重载 |
| GET/POST/PUT/DELETE | `/api/v1/namespaced/{kind}/...` | 命名空间级资源 CRUD |
| GET/POST/PUT/DELETE | `/api/v1/cluster/{kind}/...` | 集群级资源 CRUD |
| GET | `/configserver/{kind}/list` | ConfigServer 列表（供 edgion-ctl） |
| GET | `/configserver/{kind}` | ConfigServer 查询 |
| POST | `/api/v1/services/acme/{ns}/{name}/trigger` | 触发 ACME 续期 |

### AdminState

<!-- TODO: API 状态共享结构 -->

### 请求处理流程

<!-- TODO: 路由 → handler → ConfMgr 委托 → 响应 -->

### 与 edgion-ctl 的交互

<!-- TODO: 三种 target 模式（center/server/client）如何映射到不同端点 -->
