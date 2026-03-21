---
name: gateway-plugin-system
description: 插件系统架构：4 阶段执行、PluginRuntime 预构建、条件执行、28 个 HTTP 插件、Stream 插件。
---

# 插件系统

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/05-plugin-system.md`

## 待填充内容

### 四阶段执行模型

<!-- TODO:
| 阶段 | 时机 | 异步 | 签名 |
|------|------|------|------|
| RequestFilter | 上游之前 | Yes | run_request() |
| UpstreamResponseFilter | 上游 headers 之后 | No | run_upstream_response_filter() |
| UpstreamResponseBodyFilter | 每个 chunk | No | run_upstream_response_body_filter() |
| UpstreamResponse | 上游之后 | Yes | run_upstream_response() |
-->

### PluginRuntime

<!-- TODO: 在 HTTPRoute/GRPCRoute preparse 时构建（非每请求） -->

### 条件执行

<!-- TODO: ConditionalRequestFilter 自动包装，skip/run 条件 -->

### HTTP 插件列表

<!-- TODO: 28 个内置 HTTP 插件的分类和简介 -->
<!-- 认证：basic_auth, jwt_auth, key_auth, hmac_auth, ldap_auth, header_cert_auth, forward_auth, openid_connect -->
<!-- 流控：rate_limit, rate_limit_redis, bandwidth_limit, ip_restriction -->
<!-- 安全：cors, csrf, jwe_decrypt -->
<!-- 转换：proxy_rewrite, response_rewrite, request_restriction, ctx_set -->
<!-- 路由：dynamic_external_upstream, dynamic_internal_upstream, direct_endpoint -->
<!-- 可观测：debug_access_log, all_endpoint_status -->
<!-- 其他：mock, real_ip, request_mirror, dsl -->

### Stream 插件

<!-- TODO: IP 限制、TLS 路由选择 -->

### 目录布局

<!-- TODO: http/ (实现), stream/ (TCP 层), runtime/ (框架 + 条件 + GatewayAPI 适配器) -->
