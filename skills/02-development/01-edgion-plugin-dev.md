# HTTP 插件开发指南

用于这些任务：

- 开发新的 HTTP 请求/响应插件
- 调试已有 HTTP 插件的执行逻辑
- 理解插件生命周期、执行阶段和 context 传递

## 插件执行阶段

Edgion HTTP 插件在以下阶段可介入请求处理：

| 阶段 | 说明 |
|------|------|
| `request_filter` | 请求头到达，可修改/拒绝请求 |
| `upstream_peer` | 选择上游节点前（可影响路由） |
| `upstream_request` | 转发给上游前（可修改上游请求） |
| `upstream_response` | 收到上游响应后（可修改响应） |
| `response_filter` | 响应返回客户端前 |
| `logging` | 请求结束后，写入 access log |

## 开发步骤

1. **阅读主架构文档**：[../01-architecture/02-gateway/12-edgion-plugin-dev.md](../01-architecture/02-gateway/12-edgion-plugin-dev.md)
2. **选择插件模板**：在 `src/core/gateway/plugins/` 下找最接近的已有插件复制
3. **实现插件 trait**：实现 `EdgionPlugin` trait 的相关阶段方法
4. **注册插件**：在插件注册表中添加新插件的 kind 和工厂函数
5. **运行时文件**：插件运行时位于 `src/core/gateway/plugins/runtime/plugin_runtime.rs`
6. **编写配置 Schema**：定义插件配置结构体，支持从 YAML/CRD 反序列化
7. **集成测试**：在 `examples/test/conf/EdgionPlugins/` 下添加测试用例

## 日志与可观测性

- 插件内禁止使用 `tracing::` 宏（数据面热路径）
- 通过 `PluginLog` 记录执行摘要：`log.push("OK u=alice; ")`
- 敏感信息只记录前缀：`log.push(&format!("OK k={}***; ", &key[..8]))`
- 完整规范见 `skills/03-coding/` 下的可观测性文档

## 常见踩坑

- 忘记在请求路径使用 `tracing::` 导致高并发性能问题
- 插件 `deny` 后未正确设置响应状态码和 body
- 插件配置反序列化失败时未给出清晰错误，导致静默忽略

## 参考文档

- 主架构参考：[../01-architecture/02-gateway/12-edgion-plugin-dev.md](../01-architecture/02-gateway/12-edgion-plugin-dev.md)
