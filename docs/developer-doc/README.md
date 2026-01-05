# Edgion 开发者文档

欢迎来到 Edgion 开发者文档。本目录包含帮助开发者理解和扩展 Edgion 系统的文档。

## 文档列表

### [添加新资源类型指南](./add-new-resource-guide.md)

完整的指南，详细说明如何在 Edgion 中添加一个新的 Kubernetes 资源类型。包括：
- 完整的检查清单
- 详细的实施步骤
- 以 EdgionStreamPlugins 为例的实际示例
- 常见问题解答

**适用场景**：
- 添加新的 CRD 资源
- 扩展 Gateway API 支持
- 实现自定义资源类型

### [架构概览](./architecture-overview.md)

Edgion 系统的整体架构视图，包括：
- 系统架构图
- 核心模块说明
- 资源流转详解
- 请求处理流程
- 模块依赖关系
- 性能考虑和扩展点

**适用场景**：
- 理解系统整体设计
- 查找特定功能的实现位置
- 规划新功能的实现方案

### [日志系统架构](./logging-system.md)

Edgion 日志系统的设计与实现，包括：
- Access Log 和 SSL Log 架构
- 批处理写入机制
- 日志轮转策略
- 性能优化技巧
- 扩展新输出类型的方法
- 故障排查指南

**适用场景**：
- 理解日志系统的工作原理
- 优化日志性能配置
- 扩展新的日志输出类型
- 排查日志相关问题

### [Work Directory 设计](./work-directory.md)

Edgion 工作目录管理设计，包括：
- 统一路径管理机制
- 配置优先级规则
- 路径解析算法
- 目录验证流程
- 从 prefix_dir 迁移指南

**适用场景**：
- 理解路径管理机制
- 配置不同部署环境
- 排查路径相关问题
- 迁移旧代码

### [Annotations 指南](./annotations-guide.md)

Edgion 注解（Annotations）系统说明，包括：
- 注解的定义和使用
- 常用注解列表
- 如何添加新注解

**适用场景**：
- 使用注解配置高级特性
- 添加自定义注解

### [资源注册指南](./resource-registry-guide.md)

资源注册系统的说明，包括：
- 资源注册机制
- 如何注册新资源类型

**适用场景**：
- 理解资源如何被系统识别
- 注册新的资源类型

## 快速开始

### 添加新资源

如果你想添加一个新的资源类型，请按照以下步骤：

1. 阅读 [架构概览](./architecture-overview.md) 了解系统整体结构
2. 参考 [添加新资源类型指南](./add-new-resource-guide.md) 的检查清单
3. 查看现有资源的实现作为参考（如 `EdgionPlugins`、`HTTPRoute`）
4. 按照指南逐步实施
5. 运行测试确保功能正常

### 理解现有功能

如果你想理解某个现有功能的实现：

1. 从 [架构概览](./architecture-overview.md) 找到相关模块
2. 查看模块的源代码和注释
3. 参考用户文档了解功能的使用方式
4. 运行示例配置进行实验

## 代码组织

Edgion 代码按照以下结构组织：

```
src/
├── types/              # 类型定义层
│   ├── resources/      # 资源定义
│   └── resource_meta_traits/  # ResourceMeta trait 实现
├── core/               # 核心功能层
│   ├── conf_mgr/       # 配置管理
│   ├── conf_sync/      # 配置同步
│   ├── routes/         # 路由引擎
│   ├── plugins/        # 插件系统
│   ├── lb/             # 负载均衡
│   ├── backends/       # 后端管理
│   ├── tls/            # TLS 管理
│   ├── observe/        # 可观测性（日志、metrics）
│   └── link_sys/       # 日志输出基础设施
└── bin/                # 可执行程序
    ├── edgion_controller.rs
    ├── edgion_gateway.rs
    └── edgion_ctl.rs
```

## 开发规范

### 代码风格

- 使用 `cargo fmt` 格式化代码
- 使用 `cargo clippy` 检查代码质量
- 为公共 API 编写文档注释
- 遵循 Rust 命名规范

### 提交规范

- 提交信息应清晰描述变更内容
- 大的功能应分多个小提交
- 每个提交应保持代码可编译

### 测试

- 为新功能编写单元测试
- 添加集成测试验证端到端功能
- 更新示例配置

## 相关资源

### 外部文档

- [Kubernetes Custom Resources](https://kubernetes.io/docs/concepts/extend-kubernetes/api-extension/custom-resources/)
- [Gateway API Specification](https://gateway-api.sigs.k8s.io/)
- [Pingora Documentation](https://github.com/cloudflare/pingora)
- [kube-rs Documentation](https://docs.rs/kube/latest/kube/)

### 用户文档

- [用户指南](../user-guide/) - 面向最终用户的使用文档
- [示例配置](../../examples/conf/) - 各种资源的示例配置

## 贡献

欢迎贡献！如果你发现文档有误或需要改进，请：

1. 提交 Issue 描述问题
2. 或直接提交 Pull Request 修复

## 联系方式

如有问题，请通过以下方式联系：

- 提交 GitHub Issue
- 发送邮件到项目维护者

---

**最后更新**: 2025-01-05  
**版本**: Edgion v0.1.0

