# Stream 插件开发指南

用于这些任务：

- 开发新的 TCP/UDP/TLS Stream 层插件
- 调试已有 Stream 插件的连接过滤逻辑
- 理解 Stream 插件与 HTTP 插件的差异

## Stream 插件 vs HTTP 插件

| 维度 | Stream 插件 | HTTP 插件 |
|------|------------|----------|
| 协议层 | TCP/UDP/TLS 连接层 | HTTP 请求/响应层 |
| 可见信息 | SNI、源 IP、连接元数据 | HTTP headers、path、body |
| 典型用途 | IP 过滤、连接限速、SNI 路由 | 认证、改写、限流 |
| context | `StreamContext` | `HttpContext` |

## Stream 插件执行阶段

| 阶段 | 说明 |
|------|------|
| `new_connection` | 新连接建立时（TCP accept 之后） |
| `connection_filter` | 过滤/拒绝连接 |
| `connection_close` | 连接关闭时清理 |

## 开发步骤

1. **阅读主架构文档**：[../01-architecture/02-gateway/13-stream-plugin-dev.md](../01-architecture/02-gateway/13-stream-plugin-dev.md)
2. **理解 annotation 驱动的配置方式**：Stream 插件通常通过 `edgion.io/*` 注解配置，参考 `skills/02-features/05-annotations/`
3. **选择模板**：在 `src/core/gateway/stream/plugins/` 下找最接近的已有插件
4. **实现 `StreamPlugin` trait**：关注 `connection_filter` 阶段
5. **注册插件**：添加到 Stream 插件注册表
6. **编写测试**：在 `examples/test/conf/Gateway/StreamPlugins/` 下补充测试用例

## 日志规范

- 数据面热路径禁止 `tracing::` 宏
- 连接级错误写入 `ctx.err_log`
- 执行摘要写入 `PluginLog`

## 常见踩坑

- Stream 插件拿不到 HTTP 信息（body、path），这是设计上的限制
- 在 `new_connection` 阶段做耗时操作会阻塞连接建立
- 忘记在注解文档中同步新插件的 annotation key

## 参考文档

- 主架构参考：[../01-architecture/02-gateway/13-stream-plugin-dev.md](../01-architecture/02-gateway/13-stream-plugin-dev.md)
