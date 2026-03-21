---
name: core-layout
description: Core 模块分层规范：模块放置规则、目录结构约束、anti-patterns。
---

# Core 分层定版

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/09-core-layout.md`

## 概要

`src/core/` 下只有 4 个顶级组：controller/、gateway/、ctl/、common/。本文定义模块放置规则。

## 待填充内容

### 顶级分组规则

<!-- TODO: 4 组定义、放置原则 -->

### Gateway 模块布局

<!-- TODO: 按子系统划分（api, backends, cli, config, conf_sync, lb, link_sys, observe, plugins, routes, runtime, services, tls） -->

### Controller 模块布局

<!-- TODO: 按子系统划分（api, cli, conf_mgr, conf_sync, observe, services） -->

### Common 模块布局

<!-- TODO: 共享模块（conf_sync, config, matcher, utils） -->

### 放置原则

<!-- TODO: 代码归属原则 — 放在 own 的 bin 下，只有 2+ bin 依赖时才移到 common -->

### Anti-patterns

<!-- TODO: 扁平桶、顶级 shim、交叉目录 -->
