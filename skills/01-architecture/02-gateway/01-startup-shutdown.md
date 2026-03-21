---
name: gateway-startup-shutdown
description: edgion-gateway 启动与关闭流程：配置加载、gRPC 连接、Pingora 服务器创建、两阶段启动。
---

# Gateway 启动与关闭

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/03-data-plane.md`（启动部分）

## 待填充内容

### 启动流程（EdgionGatewayCli::run()）

<!-- TODO: 完整启动序列 -->
<!--
1.  加载 TOML 配置
2.  初始化工作目录
3.  初始化日志系统
4.  创建 ConfigSyncClient（连接 Controller gRPC）
5.  初始化全局 ConfigClient（缓存所有资源）
6.  从 Controller 获取 ServerInfo（endpoint 模式、支持的 kinds）
7.  开始 Watch Kubernetes 资源
8.  启动辅助服务（Backend cleaner、Health check、Admin API、Metrics API）
9.  等待所有 cache ready
10. 预加载负载均衡器
11. 初始化所有可观测性 logger
12. 创建并配置 Pingora 服务器（Phase 1）
13. 将 Tokio 运行时移到后台线程
14. 运行 Pingora 服务器（Phase 2 — 阻塞直到关闭）
-->

### Pingora 集成

<!-- TODO:
- create_and_configure_server(): 创建 Pingora ServerConf 和 bootstrap
- run_server(): 同步运行 Pingora 服务器
- cli/pingora.rs: 服务器设置函数
-->

### Listener 配置

<!-- TODO:
- GatewayBase::configure_listeners(): 将 listener 添加到 Pingora
- listener_builder.rs: 构建单个 listener
- 每个 Gateway 资源的 listener 定义
-->

### 关闭流程

<!-- TODO: Pingora 优雅关闭、Tokio 运行时清理 -->
