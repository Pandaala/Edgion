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

```
v1 可用？ → watch v1
  ↓ 否
v1beta1 可用？ → watch v1beta1
  ↓ 否
不支持，报错
```

用 Discovery API 检测集群支持的版本。

### 2. 内部类型统一

- 内部定义一个版本无关的 struct（如 `ReferenceGrant`）
- 提供 `from_v1()` / `from_v1beta1()` 转换函数
- Handler/匹配逻辑只操作内部类型，不感知 API 版本

### 3. CRD 多版本共存

若 Edgion 自行安装 CRD：
- `spec.versions` 列出所有支持版本
- `storage: true` 设在最高稳定版
- 需要 conversion webhook 做版本间转换

### 4. 测试矩阵

覆盖以下场景：
- 集群只有 beta 版本
- 集群 beta + GA 共存
- 集群只有 GA 版本

## 典型案例：ReferenceGrant v1beta1 → v1

Gateway API v1.1.0 起：
- `v1` 为 storage version
- `v1beta1` 仍 served，标记 `deprecated: true`
- 控制器优先 watch `v1`，API server 自动做版本转换
- 无字段变更，纯版本晋升

## Edgion 实施要点

1. **类型层**：`src/types/resources/` 中内部 struct 版本无关，转换层隔离 API 版本差异
2. **Controller watch**：informer 初始化时做版本探测，优先高版本
3. **渐进式迁移**：先加 v1 支持 → 双版本并存 → 下游集群全部升级后移除 beta
