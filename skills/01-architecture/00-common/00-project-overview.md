---
name: project-overview
description: Edgion 项目高层架构图、Crate 结构、三个 bin 定义、代码组织、关键依赖。
---

# 项目总览

> **状态**: 框架已建立，待填充详细内容。

## 概要

Edgion 是基于 Pingora 的 Kubernetes Gateway，采用 Controller–Gateway 分离架构，通过 gRPC 做配置同步。

## 待填充内容

### 高层架构图

<!-- TODO: 从 _01-architecture-old/00-overview.md 迁移架构总览图 -->
<!-- 包含 Controller–Gateway 分离模型、gRPC 同步、数据面代理 -->

### Crate 结构

<!-- TODO: 单 Crate 三 bin 的设计 -->
- `edgion-controller` — 控制面（Tokio 多线程运行时）
- `edgion-gateway` — 数据面（同步 Tokio + Pingora 主循环）
- `edgion-ctl` — CLI 管理工具

### 代码目录组织

<!-- TODO: src/ 目录结构说明 -->
```
src/
├── bin/              # 薄入口包装
├── core/             # 所有业务逻辑
│  ├── controller/    # 控制面代码
│  ├── gateway/       # 数据面代码
│  ├── ctl/           # CLI 代码
│  └── common/        # 共享工具
└── types/            # 纯数据定义（无业务逻辑）
```

### 关键依赖

<!-- TODO: Pingora、Tokio、Axum、Tonic、Kube、Serde、Rustls/BoringSSL 等 -->

### EdgionHttpContext

<!-- TODO: 每请求状态载体，贯穿 HTTP 生命周期 -->

### 测试基础设施

<!-- TODO: test_server、test_client、集成测试编排 -->
