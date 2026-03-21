---
name: controller-acme
description: ACME 证书自动化服务：Let's Encrypt 集成、证书签发流程、DNS 提供商、仅 Leader 运行。
---

# ACME 证书服务

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### 概述

<!-- TODO: 可选服务，通过 Let's Encrypt 自动签发和续期证书 -->

### 处理流程

<!-- TODO:
1. 用户创建/更新 EdgionAcme 资源
2. Handler 处理请求
3. ACME 客户端执行域名验证
4. 证书签发并存储到 Secret
5. Status 更新过期追踪
-->

### ACME 客户端

<!-- TODO: ACME 协议客户端，与 Let's Encrypt 通信 -->

### DNS 提供商

<!-- TODO: DNS 挑战的多种实现 -->

### Leader-only 约束

<!-- TODO: HTTP-01 挑战需要单点，因此 ACME 仅在 Leader 上运行 -->
