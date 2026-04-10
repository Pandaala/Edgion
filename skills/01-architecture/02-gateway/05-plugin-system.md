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
- `KeyMatch` — 键值精确/多值/正则匹配
- `Include` — 键值是否在指定集合中（支持 values 列表和正则）
- `Exclude` — 键值不在集合中（Include 的反向）
- `TimeRange` — 当前时间是否在指定范围内
- `Probability` — 概率性执行（如 10% 采样）

条件评估分为异步版本（`evaluate_detail`）和同步版本（`evaluate_detail_sync`），分别用于 async 和 sync 阶段。

Gateway API 标准 filter（通过 `add_from_httproute_filters` 添加的）不包装条件，始终执行。

跳过日志格式：`"skip:keyExist,hdr:X-Internal"` 或 `"!run:include,method"`。

### ⚠️ 关键约束：regex 必须在 preparse 阶段 compile

`KeyMatch`、`Include`、`Exclude` 支持 `regex` 字段。`compiled_regex` 字段标注了 `#[serde(skip)]`，反序列化后为 `None`。**必须在 preparse 阶段调用 `compile()` 完成编译**，否则每次请求命中 fallback 路径时都会执行 `Regex::new()`，造成严重 CPU 开销。

**compile 调用链**（一次性，在 gRPC list/watch 接收资源时触发）：

```
resource.pre_parse()                        # event_dispatch.rs
  → EdgionPlugins::preparse()              # edgion_plugins/mod.rs
      → compile_conditions()               # 遍历全部 entry，调 PluginConditions::compile()
          → Condition::compile()           # conditions/types.rs
              → KeyMatchCondition::compile() / IncludeCondition::compile() / ExcludeCondition::compile()
      → add_from_*()                       # plugin_runtime.rs（此时 clone 到的 conditions 已编译好）
```

**新增带 regex 的条件类型时**：在 `Condition::compile()` 的 match 臂中加入对应分支，否则该类型 regex 永远不会被编译。

## HTTP 插件列表（29 个）

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

### 路由类（4 个）

| 插件 | 模块 | 说明 |
|------|------|------|
| DynamicExternalUpstream | `dynamic_external_upstream` | 动态外部上游（运行时指定外部地址） |
| DynamicInternalUpstream | `dynamic_internal_upstream` | 动态内部上游（运行时选择 K8s Service） |
| DirectEndpoint | `direct_endpoint` | 直接指定后端端点地址 |
| RegionRoute | `region_route` | 多 region 流量路由：有序 routeRules（RouteByDirectRegion / RouteByKeyConfMatch / RouteByHashRange）+ hashKeyGet passthrough + hashCalc writeBack + myRegion（本地 pass-through）+ failoverTo（静态重定向）+ auto failover |

### 其他类（8 个）

| 插件 | 模块 | 说明 |
|------|------|------|
| Mock | `mock` | 返回模拟响应（不转发到上游） |
| DslPlugin | `dsl` | DSL 脚本引擎（Parser→AST→Compiler→Bytecode→VM，~4700 行，详见下方 DSL 引擎章节） |
| CtxSet | `ctx_set` | 设置请求上下文变量 |
| JweDecrypt | `jwe_decrypt` | JWE 加密令牌解密 |
| RequestMirrorPlugin | `request_mirror` | 请求镜像（复制请求到另一个后端） |
| RequestRestriction | `request_restriction` | 请求限制（按路径、方法等） |
| AllEndpointStatus | `all_endpoint_status` | 返回所有后端端点状态 |
| DebugAccessLogToHeader | `debug_access_log` | 将 access log 信息写入响应头（调试用，UpstreamResponseFilter 阶段） |

## DSL 脚本引擎（DslPlugin）

> 源码：`src/core/gateway/plugins/http/dsl/`（~4700 行，含 `lang/` 子目录）
> 增强设计文档：`docs/superpowers/specs/2026-03-22-dsl-plugin-enhancement-design.md`

EdgionDSL 是轻量级请求/响应规则引擎，基于自研字节码 VM，定位为"零运行时依赖、编译时安全保证、亚毫秒级执行"。

### 架构

```
源码/YAML → Parser(nom) → AST → Compiler → Bytecode(JSON/base64) → VM 执行
```

- **Parser**：nom combinator，解析预算 50k 次调用
- **AST**：节点上限 2000，嵌套深度 ≤64
- **Compiler**：类型检查、函数签名校验、正则编译期验证，输出字节码（≤256KB）
- **VM**：同步栈式 VM，`catch_unwind` 隔离 panic

### 已实现能力（39 个 Builtin）

| 类别 | API |
|------|-----|
| Request 读取（15） | `req.header`/`method`/`path`/`query`/`query_string`/`cookie`/`client_ip`/`remote_ip`/`path_param`/`header_names`/`scheme`/`host`/`uri`/`content_type`/`has_header` |
| Request 修改（6） | `req.set_header`/`append_header`/`remove_header`/`set_uri`/`set_host`/`set_method` |
| Response 修改（3） | `resp.set_header`/`append_header`/`remove_header` |
| Context KV（3） | `ctx.get`/`set`/`remove` |
| 工具函数（12） | `log`/`len`/`substr`/`to_int`/`to_str`/`to_upper`/`to_lower`/`base64_encode`/`base64_decode`/`url_encode`/`url_decode`/`time_now` |

语言特性：变量（let/let mut）、if/else、while、for-range、for-in、逻辑/比较/算术运算符、字符串方法（starts_with/ends_with/contains/matches）、`return deny(status, body)` / `return next()`。

### 沙箱安全模型

| 层 | 限制 |
|----|------|
| 解析 | 源码 ≤64KB，解析预算 50k，AST ≤2000 节点，嵌套 ≤64 |
| 编译 | 指令 ≤1000，locals ≤64，常量 ≤256，正则语法检查 |
| 运行时 | steps ≤10k，loop ≤100 次，calls ≤500，stack ≤128，string ≤8KB（均可 per-plugin 配置） |

### 待实现增强（设计已完成）

分三步实现（详见设计文档）：
1. **Step-1（P0）**：实现 5 个 stub 函数（sha256/md5/regex_find/regex_replace/range-as-expr），仅改 `builtins.rs`
2. **Step-2（P1）**：resp 读取（resp.status/resp.header/resp.has_header）+ break/continue
3. **Step-3（P2）**：实现 UpstreamResponseFilter trait，支持响应阶段执行

### 配置方式

```yaml
type: Dsl
config:
  name: "my-script"
  source: |          # 内联源码（或用 bytecode: 预编译）
    let auth = req.header("Authorization")
    if auth == nil {
      return deny(401, "Unauthorized")
    }
  errorPolicy: deny  # ignore | deny | denyWith
  vmLimits:          # 可选，覆盖默认限制
    maxSteps: 50000
```

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
