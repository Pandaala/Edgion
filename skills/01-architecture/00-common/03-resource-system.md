---
name: resource-system
description: 资源系统架构：define_resources! 宏、ResourceMeta trait、ResourceKind 枚举、Preparse 机制、资源类型全表。
---

# 资源系统

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/08-resource-system.md`

## 概要

所有资源通过 `define_resources!` 宏在 `src/types/resource/defs.rs` 中统一声明，这是唯一真相源。

## 待填充内容

### define_resources! 宏

<!-- TODO: 宏的定义位置、参数格式、生成内容 -->

### ResourceMeta trait

<!-- TODO: get_version()、resource_kind()、kind_name()、key_name()、pre_parse() -->

### ResourceKind 枚举

<!-- TODO: 自动生成的枚举，包含所有资源类型 -->

### 资源类型全表

<!-- TODO: 20 种资源的完整列表 -->
| 分类 | 资源 | 说明 |
|------|------|------|
| 核心配置 | GatewayClass | Gateway API GatewayClass |
| 核心配置 | Gateway | Gateway API Gateway |
| 核心配置 | EdgionGatewayConfig | Edgion 全局配置（集群级） |
| 路由 | HTTPRoute | HTTP/HTTPS 路由 |
| 路由 | GRPCRoute | gRPC 路由 |
| 路由 | TCPRoute | TCP 路由 |
| 路由 | TLSRoute | TLS 路由 |
| 路由 | UDPRoute | UDP 路由 |
| 后端/服务 | Service | Kubernetes Service |
| 后端/服务 | EndpointSlice | Kubernetes EndpointSlice |
| 后端/服务 | Endpoint | Kubernetes Endpoint（旧版） |
| 安全/策略 | EdgionTls | TLS 证书配置 |
| 安全/策略 | Secret | Kubernetes Secret |
| 安全/策略 | ReferenceGrant | 跨命名空间引用授权 |
| 安全/策略 | BackendTLSPolicy | 后端 TLS 策略 |
| 插件/扩展 | EdgionPlugins | HTTP 层插件定义 |
| 插件/扩展 | EdgionStreamPlugins | Stream 层插件定义 |
| 插件/扩展 | PluginMetaData | 插件元数据 |
| ACME | EdgionAcme | ACME 自动证书 |
| 基础设施 | LinkSys | 外部系统连接器 |

### Preparse 机制

<!-- TODO: 在 Controller 和 Gateway 两侧都执行的预解析，构建运行时结构 -->

### 资源同步范围

<!-- TODO: 哪些资源会同步到 Gateway，哪些不会（如 ReferenceGrant、Secret） -->
