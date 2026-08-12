# EdgionBackend 停机迁移

旧 AI 后端资源与 `EdgionBackend` 是 wire 不兼容的 kind-23 资源。不支持滚动升级、部分二进制
回滚、仅回滚 CRD 或混用新旧对象图。

## 切换

1. 导出旧 CRD、全部旧后端对象、引用它们的 HTTPRoute、ReferenceGrant、后端流量策略、相关 Secret、
   RBAC 与部署清单。resource version 和 status 只作为证据保留；另行准备干净的完整对象恢复清单。
2. 离线生成并验证完整替换图：新 CRD、嵌套的 `EdgionBackend.spec.ai` 对象、更新后的 Route 与策略
   kind、RBAC，以及配套的 Controller 和 Gateway 镜像。
3. 停止全部旧 Controller 与 Gateway。在更改 API 前，确认没有旧进程仍能 reconcile、watch 或提供
   配置同步。
4. 安装新 CRD 与 RBAC，应用完整转换后的后端、Route 和策略图，再删除旧对象与旧 CRD。不得只就地
   patch `spec`。
5. 只启动配套的新 Controller 与 Gateway 二进制。
6. 验证 discovery 仅暴露 `edgionbackends.edgion.io`；RBAC 允许新 plural 与 status 且不允许旧
   plural；严格 server dry-run 成功；Admin/config-client 使用 `edgionbackend` 路径；全部资源达到预期
   condition；代表性 AI 流量与 Secret 轮换成功。

## 回滚

1. 恢复任何旧 API 或对象前，停止全部新 Controller 与 Gateway。
2. 将新资源图、新 CRD 与新 RBAC 作为一个协调变更移除。
3. 使用完整对象恢复保存的旧 CRD/RBAC，以及旧形状的后端、Route、策略、ReferenceGrant 与部署对象。
   绝不能把新的嵌套 kind-23 payload 发送给旧二进制。
4. 只启动上一版本的配套二进制，然后验证旧 discovery、status、配置同步与代表性流量。
