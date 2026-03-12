# Edgion 开发指南

本目录面向 Edgion 贡献者，重点覆盖架构设计、资源处理链路、插件扩展和内部可观测性实现。

## 架构主线

### 1. 系统整体架构

- [架构概览](./architecture-overview.md)：控制面/数据面、模块边界、请求路径总览。
- [资源架构总览](./resource-architecture-overview.md)：资源同步和处理链路（watch/list、缓存、解析、下发）。

### 2. 资源处理与注册

- [资源注册指南](./resource-registry-guide.md)：资源类型如何接入统一注册体系。
- [添加新资源类型指南](./add-new-resource-guide.md)：新增 CRD 的完整落地步骤。

### 3. 配置扩展机制

- [Annotations 指南](./annotations-guide.md)：`edgion.io/*` 注解设计、解析和运行时行为。

### 4. 网关基础设施

- [Work Directory 设计](./work-directory.md)：工作目录解析、优先级与迁移策略。
- [日志系统架构](./logging-system.md)：Access/SSL/TCP/UDP 日志链路与输出系统。

### 5. 设计评审文档

- [JWT Auth 插件设计](./jwt-auth-plugin-design.md)：插件设计阶段文档示例（功能与配置评审）。

## 建议阅读顺序

1. [架构概览](./architecture-overview.md)
2. [资源架构总览](./resource-architecture-overview.md)
3. [资源注册指南](./resource-registry-guide.md)
4. [添加新资源类型指南](./add-new-resource-guide.md)
5. [Annotations 指南](./annotations-guide.md)
6. [日志系统架构](./logging-system.md)

## 目录定位原则

- `dev-guide`：源码内部实现、架构设计、贡献流程。
- `ops-guide`：Gateway/GatewayClass、监听器、TLS、观测、基础设施运维。
- `user-guide`：HTTPRoute/TCPRoute/GRPCRoute/UDPRoute 配置与插件使用。

如果一个主题同时涉及多类读者，请分别写独立文档并互相引用，而不是混在同一篇里。

## 文档维护最佳实践

1. 每次新增或删除文档时，同步更新对应目录 `README.md`。
2. 只链接已存在文档；规划中内容标注为“（即将推出）”。
3. 非 Gateway API 标准能力，在文档开头明确标记为 Edgion 扩展。
4. 对影响请求行为的隐式逻辑（默认值、执行顺序、自动补全）必须显式说明。
5. 用户文档和开发文档分别写：一个讲“怎么配”，一个讲“怎么实现”。
