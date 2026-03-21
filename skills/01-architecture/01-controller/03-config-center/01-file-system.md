---
name: config-center-file-system
description: FileSystemCenter 实现：本地 YAML 目录监听、文件命名规范、status 持久化。
---

# FileSystemCenter 实现

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/01-config-center/01-file-system.md`

## 待填充内容

### 运作模式

<!-- TODO: 监听本地 YAML 目录，将文件变更转换为资源事件 -->

### 目录结构与文件命名

<!-- TODO:
- {Kind}_{namespace}_{name}.yaml（命名空间级）
- {Kind}__{name}.yaml（集群级，双下划线）
- .status 文件持久化
-->

### 启动流程

<!-- TODO: 初始化时扫描目录，运行时使用 notify 库监听变更 -->

### 与 Kubernetes 实现的差异

<!-- TODO: 无 Leader 选举、status 通过 .status 文件、本地文件监听 -->
