---
name: controller-acme
description: ACME 证书自动化服务：EdgionAcme 资源驱动、Let's Encrypt 兼容客户端、DNS 提供商实现、仅 Leader 运行、Secret 集成。
---

# ACME 证书自动化服务

## 概述

ACME 服务是 Controller 的可选组件，通过 Let's Encrypt（或兼容的 ACME 服务器）自动签发和续期 TLS 证书。该服务由 EdgionAcme 自定义资源驱动，用户只需声明域名和验证方式，系统自动完成证书生命周期管理。

核心组件：

- **EdgionAcme 资源** — 声明式证书需求，定义域名、ACME 服务器、验证方式
- **ACME 协议客户端** — 与 Let's Encrypt 兼容的 ACME v2 协议实现
- **DNS 提供商** — 域名验证的多种 DNS 提供商实现

## 处理流程

```
用户创建/更新 EdgionAcme
        │
        ▼
  Handler 接收并处理请求
        │
        ▼
  ACME 客户端向 ACME 服务器发起挑战
        │
        ▼
  DNS 提供商执行域名验证（DNS-01）
  或 HTTP-01 挑战验证
        │
        ▼
  域名验证通过 → 证书签发
        │
        ▼
  证书存储到 Secret 资源
        │
        ▼
  EdgionAcme Status 更新（含过期时间）
```

### 详细步骤

1. **资源接收**: 用户创建或更新 EdgionAcme 资源，进入 Workqueue → ResourceProcessor 流水线
2. **Handler 处理**: EdgionAcme Handler 解析资源配置，提取域名列表、ACME 服务器地址、验证方式
3. **ACME 客户端交互**: 向 ACME 服务器（如 Let's Encrypt）发起证书签发请求，获取挑战令牌
4. **域名验证**: 根据配置的验证方式执行域名所有权证明（DNS-01 或 HTTP-01）
5. **证书签发**: 验证通过后，ACME 服务器签发证书，客户端下载证书链和私钥
6. **Secret 存储**: 将签发的证书和私钥写入对应的 Secret 资源，供 Gateway TLS 使用
7. **状态更新**: 更新 EdgionAcme 资源的 Status，记录证书过期时间、签发状态等信息

## ACME 协议客户端

ACME 客户端实现 RFC 8555 协议，兼容 Let's Encrypt 及其他 ACME v2 服务器。主要职责：

- 账户注册与密钥管理
- 订单创建与挑战获取
- 挑战响应提交与状态轮询
- 证书下载与解析

## DNS 提供商

DNS-01 挑战需要在域名的 DNS 记录中添加 TXT 记录以证明域名所有权。系统提供多种 DNS 提供商实现，每种提供商封装对应的 API 调用逻辑：

- 创建 `_acme-challenge.<domain>` TXT 记录
- 等待 DNS 传播
- 验证完成后清理 TXT 记录

## Leader-only 约束

ACME 服务仅在 Leader 节点上运行。原因：

- **HTTP-01 挑战**: 需要单一端点响应 ACME 服务器的 HTTP 验证请求，多实例运行会导致挑战响应不确定
- **避免重复签发**: 多个实例同时向 ACME 服务器发起请求会导致不必要的重复操作和速率限制风险
- **状态一致性**: Leader 独占处理确保证书签发流程的原子性和一致性

在 HA 模式下，当 Leader 切换时，新 Leader 接管 ACME 服务，未完成的签发流程会重新开始。

## 集成机制

### Secret 管理集成

证书签发完成后，ACME 服务将证书写入 Secret 资源。Secret 的变更会触发依赖它的其他资源（如 Gateway Listener）通过 Requeue 机制重新处理，从而实现证书的自动热更新。

### 级联 Requeue

EdgionAcme 资源的状态变更通过 RequeueChain 触发关联资源的重新处理：

- EdgionAcme 签发成功 → Secret 更新 → 依赖该 Secret 的 Gateway/Listener 重新处理
- 证书即将过期 → EdgionAcme 重新入队 → 自动续期流程启动
