# Link System 开发指南

用于这些任务：

- 开发或修改 Edgion Link System（联邦/多集群链路）
- 理解 Controller 与 Center 之间的通信机制
- 调试跨集群配置同步问题

## Link System 概述

Link System 是 Edgion 的联邦层，负责将多个 K8s 集群的网关配置汇聚到 Center，实现集中管理和配置分发。

主要组件：

| 组件 | 职责 |
|------|------|
| Controller Link Client | 上报本集群资源到 Center |
| Center Link Server | 接收并汇聚多集群配置 |
| Link gRPC Protocol | Controller↔Center 通信协议 |

## 开发步骤

1. **阅读主架构文档**：[../01-architecture/02-gateway/14-link-sys-dev.md](../01-architecture/02-gateway/14-link-sys-dev.md)
2. **理解 gRPC 协议**：查看 `proto/` 目录下的 Link 相关 proto 定义
3. **修改 Link Client**（如需扩展上报字段）：
   - 修改 proto 定义
   - 更新 `src/core/controller/link/` 下的客户端实现
4. **修改 Link Server**（如需扩展 Center 处理）：
   - 更新 `src/core/center/link/` 下的服务端实现
5. **测试链路**：Link System 有独立的集成测试套件，参考 `examples/test/conf/` 中的 LinkSys 相关配置

## 调试技巧

- Controller 和 Center 的 link 连接状态可通过 admin API 查询
- gRPC 流断开重连有退避机制，测试时注意等待重连完成
- 配置同步延迟可通过 metrics 观测

## 常见踩坑

- proto 字段新增时忘记保证向后兼容，导致旧版本 Controller 无法连接新版本 Center
- Link Client 断线重连时，全量重新上报可能产生大量 diff 事件

## 参考文档

- 主架构参考：[../01-architecture/02-gateway/14-link-sys-dev.md](../01-architecture/02-gateway/14-link-sys-dev.md)
