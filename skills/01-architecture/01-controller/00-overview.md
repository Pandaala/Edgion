---
name: controller-overview
description: edgion-controller 总体架构：核心职责、模块划分、数据流、与 Gateway 的关系。
---

# Controller 总体架构

> **状态**: 框架已建立，待填充详细内容。

## 概要

edgion-controller 是 Edgion 的控制面，基于 Tokio 多线程运行时，负责资源的接收、校验、处理和分发。

## 待填充内容

### 核心职责

<!-- TODO:
- 从 K8s API 或文件系统接收资源变更
- 校验、预解析、处理资源
- 维护资源间依赖关系
- 通过 gRPC 分发配置给 Gateway
- 管理资源状态并回写
- 提供 Admin API 供运维使用
-->

### 模块划分

<!-- TODO: 对应 src/core/controller/ 下的模块结构 -->
```
src/core/controller/
├── api/           # Admin REST API
├── cli/           # 启动入口和初始化
├── conf_mgr/      # 配置管理器核心
│   ├── conf_center/      # 配置存储后端
│   │   ├── file_system/  # 文件系统实现
│   │   └── kubernetes/   # Kubernetes 实现
│   └── sync_runtime/     # 共享同步运行时
│       ├── resource_processor/  # 资源处理器
│       │   ├── handlers/        # 每种资源的处理器
│       │   ├── ref_grant/       # 跨命名空间引用
│       │   ├── secret_utils/    # Secret 管理
│       │   └── configmap_utils/ # ConfigMap 管理
│       └── workqueue.rs         # 工作队列
├── conf_sync/     # gRPC 同步服务
│   ├── cache_server/   # 内存缓存
│   └── conf_server/    # gRPC 服务端
├── observe/       # 可观测性
└── services/      # 附加服务（ACME）
```

### 数据流总览

<!-- TODO: 从资源变更到 Gateway 同步的完整数据流 -->

### ConfMgr 门面

<!-- TODO: ConfMgr 作为统一入口，持有 Arc<dyn ConfCenter>，工厂方法选择后端 -->

### ProcessorRegistry

<!-- TODO: 全局单例注册表，register/get/requeue_all/is_all_ready -->
