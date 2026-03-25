---
name: resource-link-sys
description: LinkSys 资源：外部系统连接器定义、多种 Provider 类型、Gateway 侧客户端管理。
---

# LinkSys 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

LinkSys 是 Edgion 的自定义扩展资源，定义外部系统连接器。通过 LinkSys 可以连接 Redis、Etcd、Elasticsearch、Webhook、LocalFile 等外部系统，供 AccessLog、插件等功能使用。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs`
- 类型定义: `src/types/resources/link_sys/`（包括 mod.rs、redis.rs、etcd.rs、elasticsearch.rs、webhook.rs、common.rs）

## Controller 侧处理

### parse

无特殊处理逻辑，直接透传。LinkSys 的配置校验由类型层面的预解析（preparse）处理。

### update_status

- `Accepted`：无 validation_errors 时为 True（reason=Accepted），有错误时为 False（reason=Invalid）

## 支持的 Provider 类型

| Provider | 用途 | 配置位置 |
|---------|------|---------|
| Redis | 限流共享状态、会话存储 | `src/types/resources/link_sys/redis.rs` |
| Etcd | 分布式配置存储 | `src/types/resources/link_sys/etcd.rs` |
| Elasticsearch | 日志存储、访问日志输出 | `src/types/resources/link_sys/elasticsearch.rs` |
| Webhook | 事件回调通知 | `src/types/resources/link_sys/webhook.rs` |
| LocalFile | 本地文件输出（日志等） | `src/types/resources/link_sys/common.rs` |

## Gateway 侧处理

LinkSys 同步到 Gateway 后，由 `LinkSysStore` 管理。Gateway 根据 LinkSys 配置创建对应的 Provider 客户端（Redis 连接池、ES 客户端等），供运行时的插件和 AccessLog 功能使用。

Provider 客户端的生命周期由 LinkSysStore 管理：
- LinkSys 创建：初始化 Provider 客户端
- LinkSys 更新：重建 Provider 客户端（连接参数变更）
- LinkSys 删除：关闭 Provider 客户端、释放连接

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| LinkSys ← AccessLog | AccessLog 配置 | 引用 | AccessLog 引用 LinkSys 输出日志到外部系统 |
| LinkSys ← EdgionPlugins | EdgionPlugins | 插件配置引用 | 限流等插件引用 Redis LinkSys 做共享状态 |
