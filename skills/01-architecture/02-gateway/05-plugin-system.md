---
name: gateway-plugin-system
description: 插件系统架构：4 阶段执行、PluginRuntime 预构建、条件执行、28 个 HTTP 插件、Stream 插件。
---

# 插件系统

> 插件系统是 Edgion Gateway 的请求处理扩展机制。
> 通过 4 个执行阶段覆盖请求的完整生命周期，在 route preparse 阶段一次性构建 PluginRuntime，避免每请求开销。

## 四阶段执行模型

| 阶段 | trait | 时机 | 异步/同步 | 方法签名 | 返回值 |
|------|-------|------|-----------|----------|--------|
| RequestFilter | `RequestFilter` | 转发到上游之前 | async | `run_request(&self, session, log) -> PluginRunningResult` | 可终止请求或返回自定义响应 |
| UpstreamResponseFilter | `UpstreamResponseFilter` | 收到上游响应 headers 之后 | sync | `run_upstream_response_filter(&self, session, log) -> PluginRunningResult` | 可修改响应头、终止 |
| UpstreamResponseBodyFilter | `UpstreamResponseBodyFilter` | 每个响应 body chunk | sync | `run_upstream_response_body_filter(&self, body, end_of_stream, session, log) -> Option<Duration>` | 返回 Duration 用于带宽限制 |
| UpstreamResponse | `UpstreamResponse` | 上游响应完成之后 | async | `run_upstream_response(&self, session, log) -> PluginRunningResult` | 异步后处理 |

执行流程要点：
- 每个阶段内，插件按注册顺序依次执行
- 任一插件返回 `ErrTerminateRequest` 或 `ErrResponse` 时立即终止该阶段，后续插件不再执行
- UpstreamResponseBodyFilter 阶段当多个插件返回 Duration 时，取最大值（最严格的限流生效）
- 每个阶段执行结束后，插件日志记录到 `ctx.stage_logs` 中，包含插件名、耗时、条件跳过信息

## PluginRuntime

`PluginRuntime` 是插件执行的核心容器，持有四个阶段的插件列表：

```rust
pub struct PluginRuntime {
    request_plugins: Vec<Box<dyn RequestFilter>>,
    upstream_response_plugins: Vec<Box<dyn UpstreamResponseFilter>>,
    upstream_response_body_plugins: Vec<Box<dyn UpstreamResponseBodyFilter>>,
    upstream_response_async_plugins: Vec<Box<dyn UpstreamResponse>>,
}
```

**构建时机**：在 HTTPRoute/GRPCRoute preparse 阶段构建，而非每个请求时构建。

构建路径：
- `PluginRuntime::from_httproute_filters()` — 处理 Gateway API 标准 filter（RequestHeaderModifier、ResponseHeaderModifier、RequestRedirect、URLRewrite、RequestMirror、ExtensionRef）
- `PluginRuntime::from_grpcroute_filters()` — 处理 GRPCRoute filter（RequestHeaderModifier、ResponseHeaderModifier、ExtensionRef）
- `add_from_request_filters()` — 处理 EdgionPlugins CRD 定义的 RequestFilter 条目
- `add_from_upstream_response_filters()` — 处理 EdgionPlugins CRD 的 UpstreamResponseFilter 条目
- `add_from_upstream_response_body_filters()` — 处理 EdgionPlugins CRD 的 UpstreamResponseBodyFilter 条目
- `add_from_upstream_responses()` — 处理 EdgionPlugins CRD 的 UpstreamResponse 条目

每个 `add_from_*` 方法返回 `Vec<String>` 验证错误列表，用于在 preparse 阶段报告配置问题。

**Clone 语义**：`PluginRuntime` 的 `clone()` 返回空实例（因为它在 preparse 时会重新构建）。

## 条件执行

EdgionPlugins 定义的插件通过 `Conditional*` wrapper 自动包装，支持条件判断：

| 包装类型 | 包装对象 |
|----------|----------|
| `ConditionalRequestFilter` | `RequestFilter` |
| `ConditionalUpstreamResponseFilter` | `UpstreamResponseFilter` |
| `ConditionalUpstreamResponseBodyFilter` | `UpstreamResponseBodyFilter` |
| `ConditionalUpstreamResponse` | `UpstreamResponse` |

条件模型 (`PluginConditions`)：
- **skip** 条件：满足任一条件则跳过插件执行
- **run** 条件：必须满足所有条件才执行插件，否则跳过

条件类型包括：
- `KeyExist` — 键是否存在（如 header、query param）
- `Include` — 键值是否在指定集合中（支持 values 列表和正则）
- `Probability` — 概率性执行（如 10% 采样）

条件评估分为异步版本（`evaluate_detail`）和同步版本（`evaluate_detail_sync`），分别用于 async 和 sync 阶段。

Gateway API 标准 filter（通过 `add_from_httproute_filters` 添加的）不包装条件，始终执行。

跳过日志格式：`"skip:keyExist,hdr:X-Internal"` 或 `"!run:include,method"`。

## HTTP 插件列表（28 个）

### 认证类（8 个）

| 插件 | 模块 | 说明 |
|------|------|------|
| BasicAuth | `basic_auth` | HTTP Basic 认证 |
| JwtAuth | `jwt_auth` | JWT 令牌验证 |
| KeyAuth | `key_auth` | API Key 认证（header/query） |
| LdapAuth | `ldap_auth` | LDAP 目录认证 |
| HmacAuth | `hmac_auth` | HMAC 签名验证 |
| HeaderCertAuth | `header_cert_auth` | 基于请求头中证书信息的认证 |
| OpenidConnect | `openid_connect` | OpenID Connect 认证 |
| ForwardAuth | `forward_auth` | 将认证请求转发到外部服务 |

### 安全类（3 个）

| 插件 | 模块 | 说明 |
|------|------|------|
| Cors | `cors` | CORS 跨域资源共享策略 |
| Csrf | `csrf` | CSRF 令牌验证 |
| IpRestriction | `ip_restriction` | IP 黑白名单限制 |

### 流量控制类（3 个）

| 插件 | 模块 | 说明 |
|------|------|------|
| RateLimit | `rate_limit` | 本地内存限流 |
| RateLimitRedis | `rate_limit_redis` | 基于 Redis 的分布式限流（Lua 脚本） |
| BandwidthLimit | `bandwidth_limit` | 带宽限制（UpstreamResponseBodyFilter 阶段） |

### 请求/响应转换类（3 个）

| 插件 | 模块 | 说明 |
|------|------|------|
| ProxyRewrite | `proxy_rewrite` | 代理路径/Host/header 重写 |
| ResponseRewrite | `response_rewrite` | 响应头修改（UpstreamResponseFilter 阶段） |
| RealIp | `real_ip` | 从 X-Forwarded-For 等头部提取真实客户端 IP |

### 路由类（3 个）

| 插件 | 模块 | 说明 |
|------|------|------|
| DynamicExternalUpstream | `dynamic_external_upstream` | 动态外部上游（运行时指定外部地址） |
| DynamicInternalUpstream | `dynamic_internal_upstream` | 动态内部上游（运行时选择 K8s Service） |
| DirectEndpoint | `direct_endpoint` | 直接指定后端端点地址 |

### 其他类（8 个）

| 插件 | 模块 | 说明 |
|------|------|------|
| Mock | `mock` | 返回模拟响应（不转发到上游） |
| DslPlugin | `dsl` | DSL 脚本引擎（内含 AST 解析器、编译器、VM） |
| CtxSet | `ctx_set` | 设置请求上下文变量 |
| JweDecrypt | `jwe_decrypt` | JWE 加密令牌解密 |
| RequestMirrorPlugin | `request_mirror` | 请求镜像（复制请求到另一个后端） |
| RequestRestriction | `request_restriction` | 请求限制（按路径、方法等） |
| AllEndpointStatus | `all_endpoint_status` | 返回所有后端端点状态 |
| DebugAccessLogToHeader | `debug_access_log` | 将 access log 信息写入响应头（调试用，UpstreamResponseFilter 阶段） |

## Stream 插件

Stream 插件在 TCP/UDP 层运行，采用两阶段架构：

| 阶段 | 位置 | 时机 | 插件 |
|------|------|------|------|
| Stage 1: ConnectionFilter | `connection_filter_bridge.rs` | pre-TLS, 仅 IP 信息 | `StreamIpRestriction` |
| Stage 2: TlsRoute | `tls_route/` | TLS 握手后, 路由匹配后 | `TlsRouteIpRestriction` |

- `StreamPlugin` trait：定义 `run_connection_filter(ctx) -> StreamPluginResult` 接口
- `TlsRoutePlugin` trait：定义 TLS 路由级别的插件接口
- `StreamPluginRuntime`：管理 ConnectionFilter 阶段的插件链
- `TlsRoutePluginRuntime`：管理 TLS 路由阶段的插件链
- `StreamPluginStore`：通过 ConfHandler 接收 EdgionPlugins 资源变更

## 目录布局

```
src/core/gateway/plugins/
├── http/                          # HTTP 插件实现（28 个）
│   ├── basic_auth/                # 各插件子目录
│   ├── jwt_auth/
│   ├── cors/
│   ├── dsl/                       # DSL 引擎（含 lang/ 子目录：AST、编译器、VM）
│   ├── ...
│   ├── common/                    # 插件共用工具（auth_common, jwt_common）
│   ├── conf_handler_impl.rs       # EdgionPlugins ConfHandler 实现
│   └── plugin_store.rs            # HTTP 插件资源存储
├── stream/                        # TCP/UDP 层插件
│   ├── ip_restriction/            # Stream IP 限制实现
│   ├── tls_route/                 # TLS 路由级插件
│   │   ├── ip_restriction.rs      # TLS 路由 IP 限制
│   │   ├── tls_route_plugin_runtime.rs
│   │   └── tls_route_plugin_trait.rs
│   ├── connection_filter_bridge.rs # Pingora ConnectionFilter 桥接
│   ├── stream_plugin_runtime.rs   # Stream 插件运行时
│   ├── stream_plugin_store.rs     # Stream 插件资源存储
│   └── stream_plugin_trait.rs     # StreamPlugin trait 定义
└── runtime/                       # 插件框架
    ├── plugin_runtime.rs          # PluginRuntime 核心（4 阶段管理 + 插件创建工厂）
    ├── conditional_filter.rs      # 条件包装器（4 种 Conditional* 类型）
    ├── conditions/                # 条件评估引擎（KeyExist, Include, Probability）
    ├── session_adapter.rs         # PingoraSessionAdapter（Pingora Session → PluginSession 适配）
    ├── log.rs                     # 插件执行日志（PluginLog, StageLogs）
    ├── traits/                    # 4 个阶段的 trait 定义
    │   ├── request_filter.rs
    │   ├── upstream_response_filter.rs
    │   ├── upstream_response_body_filter.rs
    │   ├── upstream_response.rs
    │   └── session.rs             # PluginSession trait（统一会话抽象）
    └── gateway_api_filters/       # Gateway API 标准 filter 实现
        ├── request_header_modifier.rs
        ├── response_header_modifier.rs
        ├── request_redirect.rs
        ├── url_rewrite.rs
        ├── extension_ref.rs       # ExtensionRef（递归引用 EdgionPlugins）
        ├── cors_filter.rs
        └── debug_access_log.rs
```
