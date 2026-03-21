---
name: gateway-observe
description: Gateway 可观测性：AccessLog（零拷贝 JSON）、协议日志、Prometheus Metrics。
---

# Gateway 可观测性

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### Access Log

<!-- TODO:
observe/access_log/
- 零拷贝 JSON 设计，直接从 EdgionHttpContext 构建
- 字段结构
- PluginLog 格式
-->

### 协议日志

<!-- TODO:
observe/logs/
├── SSL 日志
├── TCP 日志
├── TLS 日志
└── UDP 日志
-->

### Prometheus Metrics

<!-- TODO:
observe/metrics/
- Metrics API (:5901)
- 请求计数、延迟、后端状态等
-->

### Access Log Store（测试模式）

<!-- TODO: observe/access_log_store/ — 内存存储，供集成测试使用 -->
