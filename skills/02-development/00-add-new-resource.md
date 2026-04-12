# 添加新资源类型

用于这些任务：

- 向 Edgion 添加新的 CRD 资源类型（如新路由、新策略、新配置对象）
- 理解 Controller 侧资源处理流程和代码结构
- 参考已有资源模式选择最合适的实现模板

## 快速选模式

拿到新资源需求后，先判断它属于哪种模式：

| 模式 | 适用场景 |
|------|---------|
| `route-like` | 携带路由规则、后端引用，需要 Gateway 同步 |
| `controller-only` | 仅 Controller 处理，不推送到 Gateway |
| `plugin-like` | 插件配置对象，生命周期跟随路由/Gateway |
| `cluster-scoped` | 集群级别（非 namespace 级别）资源 |

## 实现步骤概览

1. **阅读主指南**：先读 [../01-architecture/01-controller/09-add-new-resource/00-guide.md](../01-architecture/01-controller/09-add-new-resource/00-guide.md)，确定资源类型和模式
2. **选择参考模板**：在 `skills/01-architecture/01-controller/09-add-new-resource/` 目录下选择最接近的模式文件
3. **理解资源系统**：阅读 [../01-architecture/00-common/03-resource-system.md](../01-architecture/00-common/03-resource-system.md) 了解 resource key、版本管理、事件分发机制
4. **实现 Controller 侧**：
   - 定义 CRD 结构体（spec/status）
   - 实现 reconciler 逻辑
   - 注册到 Config Center
5. **实现 Gateway 侧**（如需同步）：
   - 注册 gRPC 接收处理
   - 实现配置应用逻辑
6. **编写集成测试**：参考 [../05-testing/01-integration-testing.md](../05-testing/01-integration-testing.md)

## 常见踩坑

- 忘记在 Config Center 注册新 kind，导致 gateway 日志出现 `Unknown kind`
- route-like 资源忘记实现 `GatewayRef` 过滤，导致跨 Gateway 资源泄漏
- 新资源的 finalizer 未正确注册，删除时残留

## 参考文档

- 主指南：[../01-architecture/01-controller/09-add-new-resource/00-guide.md](../01-architecture/01-controller/09-add-new-resource/00-guide.md)
- 资源系统架构：[../01-architecture/00-common/03-resource-system.md](../01-architecture/00-common/03-resource-system.md)
- 集成测试：[../05-testing/01-integration-testing.md](../05-testing/01-integration-testing.md)
