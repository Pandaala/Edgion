---
name: api-version-upgrade
description: K8s API version lifecycle and controller-side upgrade strategy (alpha/beta/GA transitions)
---

# API Version Upgrade Strategy

Gateway API 及其他 K8s 资源从 alpha → beta → GA 的版本升级策略。

## K8s API 版本生命周期

| 阶段变更 | 兼容性保证 | 废弃策略 |
|---------|-----------|---------|
| alpha → alpha | 无，可破坏性变更 | 可直接移除 |
| alpha → beta | 需迁移指南，可重命名/移除字段 | 废弃后保留 3 个次版本 |
| beta → GA | 不可移除/重命名字段 | beta 废弃后保留 ≥3 次版本（≥9 月） |
| GA → GA (v1→v2) | 极少发生，长期共存 | — |

## 控制器侧升级策略

### 1. 版本探测 + 回退

优先 watch 最高稳定版本，集群不支持时 fallback：

```text
v1 可用？ → watch v1
  ↓ 否
v1beta1 / v1alpha3 可用？ → watch 旧版本
  ↓ 否
不支持，报错
```

用 Discovery API 或一次轻量 `list(limit=1)` 探测集群支持的 served version。

默认策略：

- 同一资源在任一时刻只选择一个版本建立 watcher
- 优先 watch 最新、最稳定、和内部语义对齐的版本
- 只有新版本不可用时，才 fallback 到旧版本
- 不要默认同时启动 `v1` 和 `v1alpha3` 两个 watcher

原因：

- 不同 API version 的 watch 是不同输入流
- 若集群同时 served 多个版本，同时 watch 可能把同一个逻辑对象重复送进 controller
- 多版本兼容的目标是“支持多个输入版本”，不是“同时消费多个版本”

### 2. API Server 转换的含义

对同一个多版本 CRD：

- API Server 按 storage version 持久化对象
- 客户端访问哪个 served version，API Server 就把对象转换成哪个版本返回
- 因此，若 CRD 已正确声明多版本并具备转换能力，controller 通常只需 watch 主版本

实务判断：

- 如果新旧版本只是版本晋升或少量兼容字段差异，优先依赖 API Server 转换
- 只有在 API Server 无法可靠转换、或者旧新版本字段差异过大时，才考虑多版本 watch

### 3. 内部类型统一

- 内部定义一个版本无关的 canonical struct
- 提供旧版本 → canonical、新版本 → canonical 的转换函数
- Handler、索引、缓存键、Gateway sync、状态计算只操作 canonical 类型
- 旧版本输入只是兼容层，不应把业务逻辑复制两份

### 4. 读写路径一致性

多版本兼容不只影响 watch，也影响 CRUD：

- preflight probe 需要按同样的优先级探测版本
- create/update/get/list/delete 应命中和 watch 相同的主版本
- 发往 API Server 的 payload `apiVersion` 要和选中的 endpoint 对齐
- status patch 也要沿用同样的版本选择

否则会出现“watch 看 v1，写却还打到 v1alpha3”的不一致。

### 5. CRD 多版本共存

若 Edgion 自行安装 CRD：

- `spec.versions` 列出所有支持版本
- `storage: true` 设在最高稳定版
- 旧版本可以 `served: true` 一段时间，作为迁移窗口
- 需要 conversion webhook 时，让 API Server 负责统一转换

注意：

- 同一 Gateway API release 下，`standard-install.yaml` 和 `experimental-install.yaml` 对同一资源暴露的 served versions 可能不同
- 不要假设“同一个 release 的同一个资源，在 standard 和 experimental 下版本完全一致”

### 6. 测试矩阵

至少覆盖：

- 集群只有旧版本
- 集群旧版 + 新版共存
- 集群只有新版本
- watch / preflight / CRUD 版本选择一致
- fallback 版本下 status patch 仍可正常写回

## 非 Kubernetes 输入源

对于 FileSystem、静态目录、测试夹具等非 K8s 输入源，策略与 KubernetesCenter 不同：

- 没有 API Server，也没有 CRD conversion 可依赖
- 不能靠运行时 probe + 单活 watch 来完成版本兼容
- 应在“读取文件内容”的最前面识别 `apiVersion` 和 `kind`
- 旧版本对象先转换成内部 canonical 类型，再进入 raw store、workqueue、handler
- 队列和业务逻辑只处理 canonical 类型，不为旧版本单独起 controller

推荐流程：

```text
read file
  -> inspect apiVersion/kind
  -> deserialize legacy or current type
  -> convert to canonical internal type
  -> raw store / queue / handler
```

这样即使同一目录中同时存在 `v1` 和 `v1alpha3` 文件，内部处理链路仍保持单一模型。

## 典型案例：ReferenceGrant v1beta1 → v1

Gateway API v1.1.0 起：

- `v1` 为 storage version
- `v1beta1` 仍 served，标记 `deprecated: true`
- 控制器优先 watch `v1`，API Server 自动做版本转换
- 无字段变更，纯版本晋升

## 典型案例：BackendTLSPolicy v1alpha3 → v1

在 Gateway API 较新的版本中：

- `BackendTLSPolicy` 的主版本已经是 `v1`
- 部分安装清单/频道仍可能继续 served `v1alpha3`
- controller 应优先 watch `v1`，只有 `v1` 不可用时才 fallback `v1alpha3`
- 旧版本 watcher 的输入应先转换成统一的 `BackendTLSPolicy` canonical 类型，再复用同一套 handler
- KubernetesStorage / Admin API / preflight 也要沿用同一版本选择策略
- FileSystem 模式下则应在 YAML 解析入口识别 `v1`/`v1alpha3`，统一转换成 `BackendTLSPolicy (v1 语义)` 后再入队

## 典型案例：TLSRoute v1alpha3 → v1

Gateway API v1.5.0 起：

- `TLSRoute` 毕业到 Standard channel，主版本 `v1`
- v1.4.0 experimental 安装清单中，storage version 仍是 `v1alpha3`
- v1alpha3 → v1 是纯版本晋升，spec 字段结构完全相同，无任何差异
- controller 应优先 watch `v1`，只有 `v1` 不可用时才 fallback `v1alpha3`
- 复用 BackendTLSPolicy 已验证的多版本模式：Kubernetes 模式"优先 v1、回退 v1alpha3、单活 watch"；FileSystem 模式"前置解析归一化到 canonical v1"
- KubernetesStorage / Admin API / preflight 也沿用同一版本选择策略

## Edgion 实施要点

1. **类型层**：`src/types/resources/` 中内部 struct 版本无关，转换层隔离 API 版本差异
2. **Controller watch**：初始化时做版本探测，优先高版本，单活 watch
3. **路径一致**：watch、preflight、storage、status patch 使用同一版本选择
4. **渐进式迁移**：先加新版本支持 → 多版本 served 共存 → 下游集群完成迁移后移除旧版本
