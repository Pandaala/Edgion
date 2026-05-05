# HTTP Plugin Development

用于这些任务：

- 新增或扩展 `EdgionPlugins` HTTP 层插件
- 调整 `PluginRuntime`、`ExtensionRef`、`PluginSession`、条件执行或 PluginLog
- 排查“插件配置有了，但路由没执行 / 执行顺序不对 / Secret 没解析 / access log 不对”

## 先读这些真实入口

- [../../../src/types/resources/edgion_plugins/mod.rs](../../../src/types/resources/edgion_plugins/mod.rs)
- [../../../src/types/resources/edgion_plugins/entry.rs](../../../src/types/resources/edgion_plugins/entry.rs)
- [../../../src/types/resources/edgion_plugins/edgion_plugin.rs](../../../src/types/resources/edgion_plugins/edgion_plugin.rs)
- [../../../src/core/gateway/plugins/runtime/plugin_runtime.rs](../../../src/core/gateway/plugins/runtime/plugin_runtime.rs)
- [../../../src/core/gateway/plugins/runtime/conditional_filter.rs](../../../src/core/gateway/plugins/runtime/conditional_filter.rs)
- [../../../src/core/gateway/plugins/runtime/traits/session.rs](../../../src/core/gateway/plugins/runtime/traits/session.rs)
- [../../../src/core/gateway/plugins/runtime/log.rs](../../../src/core/gateway/plugins/runtime/log.rs)
- [../../../src/core/gateway/plugins/http/mod.rs](../../../src/core/gateway/plugins/http/mod.rs)
- [../../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs](../../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs)
- [../../../src/core/gateway/plugins/http/conf_handler_impl.rs](../../../src/core/gateway/plugins/http/conf_handler_impl.rs)
- [../../../src/core/gateway/plugins/http/plugin_store.rs](../../../src/core/gateway/plugins/http/plugin_store.rs)
- [../../../src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs](../../../src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs)

## 关键心智模型

HTTP 插件现在有两条不同的 runtime 构建路径，先分清再改：

### 1. `EdgionPlugins` 资源路径

链路是：

1. Controller `EdgionPluginsHandler.preparse()` 调 `ep.preparse()`
2. `EdgionPlugins::preparse()` 构建 `spec.plugin_runtime` 并收集 `preparse_errors`
3. Gateway `PluginStore` 在 `full_set` / `partial_update` 时也会再次 `preparse()`
4. 运行时通过 `PluginStore` 取到资源，再执行它自己的 `plugin_runtime`

这条路径主要服务：

- `EdgionPlugins` 这种可复用插件资源
- `ExtensionRef` 嵌套调用
- 依赖 Secret 解析的 HTTP 插件

### 2. Route/Gateway API filter 路径

链路是：

- `HTTPRoute` / `GRPCRoute` 预解析时构建 `PluginRuntime::from_httproute_filters()` 或 `from_grpcroute_filters()`

这条路径主要服务：

- `RequestHeaderModifier`
- `ResponseHeaderModifier`
- `RequestRedirect`
- `URLRewrite`
- `RequestMirror`
- `ExtensionRef` wrapper 本身

最容易踩坑的是把这两条路径混在一起：

- 改了 `EdgionPlugin` enum，但没接 `PluginRuntime::create_*_from_edgion()`，`EdgionPlugins` 资源不会执行
- 只看 route filter 预解析，忽略了 `PluginStore` 和 `ExtensionRef`
- 只改 Gateway 侧，没让 controller `preparse()` / `update_status()` 暴露校验问题

## 插件类型抉择

| 场景 | 选 `EdgionPlugin` | 选 Gateway API filter |
|------|-------------------|-----------------------|
| 需要访问 Header / Cookie / Query / Path | 是 | 否 |
| 认证、限流、重写、镜像等 HTTP 语义 | 是 | 否 |
| 上游响应头 / 响应体 chunk | 是 | 否 |
| 跨插件 `PluginSession` ctx 变量 | 是 | 否 |
| 标准 Gateway API filter（HeaderModifier、Redirect、URLRewrite、Mirror） | 否 | 是，落 `gateway_api_filters/` |
| 只依赖 IP / 监听端口 / SNI / mTLS | 否 | 否，转 Stream plugin |

> 相关：[13-stream-plugin-dev.md](13-stream-plugin-dev.md)

## 选择插件阶段

| 阶段 | trait | 何时运行 | 常见用途 |
|------|-------|----------|----------|
| 请求阶段 | `RequestFilter` | 发往上游前 | Auth、RateLimit、Rewrite、Mirror、直接拒绝 |
| 响应头阶段 | `UpstreamResponseFilter` | 上游响应头到达时 | 改响应头、基于 header 快速判断 |
| 响应体阶段 | `UpstreamResponseBodyFilter` | 每个 body chunk | 带宽限制、chunk 检查 |
| 响应结束阶段 | `UpstreamResponse` | 完整响应结束后 | 少数需要完整响应上下文的逻辑 |

默认优先选 `RequestFilter`。  
如果不用上游响应信息，就不要把逻辑推到更晚的阶段。

## 最小改动清单

### 普通 HTTP 插件

通常至少会碰这些位置：

1. `src/types/resources/edgion_plugins/plugin_configs/<your_plugin>.rs`
2. `src/types/resources/edgion_plugins/plugin_configs/mod.rs`
3. `src/types/resources/edgion_plugins/mod.rs`
4. `src/types/resources/edgion_plugins/edgion_plugin.rs`
5. `src/core/gateway/plugins/http/<your_plugin>/mod.rs`
6. `src/core/gateway/plugins/http/<your_plugin>/plugin.rs`
7. `src/core/gateway/plugins/http/mod.rs`
8. `src/core/gateway/plugins/runtime/plugin_runtime.rs`

### 如果插件依赖 Secret

还要继续看：

- [../../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs](../../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs)

当前这个 handler 已经负责：

- `BasicAuth`
- `JwtAuth`
- `JweDecrypt`
- `HeaderCertAuth`
- `HmacAuth`
- `KeyAuth`
- `OpenidConnect`

如果你的新插件也依赖 Secret，要把 Secret 解析和 `secret_ref_manager` 引用注册补进去。

## 推荐开发顺序

### 1. 先决定插件类型（参见上方抉择表）

### 2. 定义配置结构

位置通常在：

- `src/types/resources/edgion_plugins/plugin_configs/`

配置结构要注意：

- `#[serde(rename_all = "camelCase")]`
- runtime 派生字段用 `#[serde(skip)]` 或 `#[schemars(skip)]`
- 如果配置可能结构性无效，实现 `get_validation_error()`

如果能复用已有配置，不要复制近似 schema。

### 3. 接入 `EdgionPlugin` enum

修改：

- [../../../src/types/resources/edgion_plugins/edgion_plugin.rs](../../../src/types/resources/edgion_plugins/edgion_plugin.rs)

至少补齐：

- enum variant
- `type_name()`
- 相关 re-export

### 4. 实现插件

位置通常是：

- `src/core/gateway/plugins/http/<your_plugin>/mod.rs`
- `src/core/gateway/plugins/http/<your_plugin>/plugin.rs`

当前仓库里比较好的参考：

- 最简单 request 插件：`mock`
- 依赖 ctx 变量：`real_ip`、`ctx_set`
- 依赖 Secret 和复杂校验：`jwt_auth`、`openid_connect`、`key_auth`
- body filter：`bandwidth_limit`

### 5. 在 `PluginRuntime` 注册构造逻辑

必须看：

- [../../../src/core/gateway/plugins/runtime/plugin_runtime.rs](../../../src/core/gateway/plugins/runtime/plugin_runtime.rs)

当前真实入口不是旧文档里的 `runtime.rs`，而是这个文件。

常见需要改的函数：

- `create_request_filter_from_edgion()`
- `create_upstream_response_filter_from_edgion()`
- `create_upstream_response_from_edgion()`
- `create_upstream_response_body_filter_from_edgion()`
- `get_plugin_validation_error()`
- `get_plugin_name()`

只改 enum、不改这些构造函数，Gateway 不会执行你的插件。

### 6. 如果插件依赖 Secret，补 controller 解析链路

当前 Secret 解析不是在 runtime 内部偷偷做的，而是在：

- `EdgionPluginsHandler.parse()`

这里负责：

- 解析 Secret
- 把结果写入 `resolved_*` runtime 字段
- 向 `secret_ref_manager` 注册级联引用
- 删除时通过 `on_delete()` 清理引用

如果你的插件依赖 Secret，但没接这条链路，通常会出现：

- YAML 能 apply
- status 可能通过
- 运行时却始终拿不到真实凭据

### 7. 最后再补用户文档和测试

如果插件是用户可见能力，除了 skill / dev-guide，还要考虑 user-guide：

- `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/`
- `docs/en/user-guide/http-route/filters/edgion-plugins/`

## 条件执行与 `ctx` 变量

当前 `RequestFilterEntry` / `UpstreamResponseFilterEntry` / 其他 entry 已经支持：

- `enable`
- `conditions`

条件执行的真实包装器在：

- [../../../src/core/gateway/plugins/runtime/conditional_filter.rs](../../../src/core/gateway/plugins/runtime/conditional_filter.rs)

重要区别：

- `EdgionPlugins` 里的条目会自动包上 `Conditional*` wrapper
- 通过 `from_httproute_filters()` / `from_grpcroute_filters()` 加进来的标准 Gateway API filters 当前不会自动套这个条件包装

跨插件传值优先使用：

- `session.set_ctx_var()`
- `session.get_ctx_var()`
- `session.remove_ctx_var()`
- `key_get()` / `key_set()`

不要为了读前置插件结果，重复解析整份请求。

## `ExtensionRef` 和嵌套执行

如果插件是通过 `ExtensionRef` 被 route 引用，重点看：

- [../../../src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs](../../../src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs)

当前行为是：

- 运行时从 `PluginStore` 按 `namespace/name` 取 `EdgionPlugins`
- 使用 `spec.plugin_runtime` 执行
- 通过深度限制和引用栈防循环
- 把嵌套插件日志压入 `edgion_plugins` 专用日志结构

如果你在调试“ExtensionRef 没生效 / 嵌套插件日志不见了 / 递归引用”，先从这里追。

## 读取请求 Path

> 架构权威文档：[uri-normalization/SKILL.md](uri-normalization/SKILL.md)

path normalization 启用后，session trait 提供 5 个 path/uri 读方法。选错视图会导致 ACL 绕过或审计信息丢失。

| 方法 | 视图 | 返回类型 | 何时使用 |
|------|------|----------|----------|
| `session.get_path()` | current（规范化） | `&str` | ACL 决策、路由匹配（**首选**，零额外 alloc） |
| `session.get_request_path()` | entry（冻结） | `&str` | 审计、安全调查、需要客户端原始 path 的场景 |
| `session.get_uri()` | current（规范化） | `String` | 需要 path + query 的完整 URI（1 alloc） |
| `session.get_request_uri()` | entry（冻结） | `String` | 审计完整 URI（入口 raw，含 query） |
| `session.get_query()` | current | `Option<String>` | 取当前 query（不规范化） |

**默认选 `get_path()`**。只有明确需要客户端原始形态时才用 `get_request_path()` / `get_request_uri()`。

## 修改请求 Path（Rewrite 插件）

**唯一正确写法**：使用 trait 方法 `session.set_upstream_uri(&new_uri)`。

```rust
// ✅ 正确：通过 trait 方法改写 path
let new_uri = format!("/v2{}", session.get_path());
session.set_upstream_uri(&new_uri)?;

// ❌ 严禁：直接操作 pingora 原始字段
// session.req_header_mut().set_uri(parsed_uri);  // 不可用！
```

**为什么禁止裸调 `req_header_mut().set_uri()`**：

- ctx 缓存（`normalized_current_path`）不会自动刷新
- 后续 `session.get_path()` 返回 **stale**（旧 path）
- access_log `$uri` 记录旧 path
- 路由决策、ACL 插件读到错误 path
- **(v4)** `ctx.uri_finalized` flag 不会被置位，导致 `pg_upstream_peer::upstream_peer_http` 单点决策可能再次写入，覆盖 rewrite 结果

`session.set_upstream_uri()` 内部实现（`session_adapter::set_upstream_uri`）自动调用 `ctx.refresh_normalized_path()` **并置 `ctx.uri_finalized = true`**，保证缓存一致 + 单点决策跳过重写。所有 rewrite 插件（`proxy_rewrite`、`url_rewrite`、`request_redirect`、DSL `req.set_uri` 等）已走此路径，**插件层零改造**。

**(v4) `req_header_mut().set_uri()` 在整个 codebase 仅 2 处合法写入**（lint Rule 2）：

1. `session_adapter::set_upstream_uri`（封装层，rewrite 插件入口）
2. `pg_upstream_peer::upstream_peer_http` 单点决策块（spec §14，写 normalized 终态到 upstream）

两处都必须紧随 `ctx.uri_finalized = true`。任何第 3 处都是违规——审查时强报，详见 `skills/04-review/uri-normalization/SKILL.md` Rule 2。

**(v4) Path 转发到 upstream 的视图决策**（如果你的插件涉及上游路径选择）：

`upstream_peer_http` 入口处单点决定上游收到 raw 还是 normalized path，5 通道优先级：

1. `ctx.uri_finalized` 已 true（rewrite 插件已写）→ 不动
2. HTTPRoute 注解 `edgion.io/forward-raw-path: "true"|"false"` → 强制
3. `external_jump.preserve_raw_path = true`（region_route 类网关互转）→ raw
4. 全局 `forward_normalized_path = false` → raw
5. 默认 → 写 normalized

**插件交互要点**：
- 你的 rewrite-class 插件输出（`set_upstream_uri`）**最高优先级**——单点决策见 `uri_finalized = true` 即跳过
- 设置 `external_jump`（如 region_route / dynamic_external_upstream 类）的插件需要明确选择 `preserve_raw_path`：网关互转选 `true`，普通外部 server 选 `false`
- 详见 spec `docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §14

## `PluginSession` 和常用 API

最常用的读写入口在：

- [../../../src/core/gateway/plugins/runtime/traits/session.rs](../../../src/core/gateway/plugins/runtime/traits/session.rs)

高频 API：

- `header_value()` / `request_headers()`
- `get_path()` / `get_request_path()` / `get_uri()` / `get_request_uri()` / `get_query()`
- `client_addr()` / `remote_addr()` / `client_cert_info()`
- `set_request_header()` / `append_request_header()` / `remove_request_header()`
- `set_upstream_uri()` / `set_upstream_host()` / `set_upstream_method()`
- `write_response_header()` / `write_response_body()` / `shutdown()`
- `set_ctx_var()` / `get_ctx_var()`
- `key_get()` / `key_get_local()` / `key_set()`

如果插件会调用外部 HTTP 服务，优先复用：

- `src/core/gateway/plugins/http/common/http_client.rs`
- `src/core/gateway/plugins/http/common/auth_common.rs`
- `src/core/gateway/plugins/http/common/jwt_common.rs`

## `PluginLog` 规则

入口在：

- [../../../src/core/gateway/plugins/runtime/log.rs](../../../src/core/gateway/plugins/runtime/log.rs)

规则保持简单：

- 记录结果，不记录长篇内部过程
- 用短语和分号结尾，例如 `OK u=jack; `
- verbose 调试用 `tracing::debug!()`，不要把 access log 当调试 dump
- 条件跳过 `cond_skip` 由 conditional wrapper 自动写，不要重复造自己的格式

## `ErrTerminateRequest` — write the response first

When a plugin returns `ErrTerminateRequest`, it **SHOULD** write a complete
response (status + headers + body) to the session before returning:

```rust
// Correct: write response, then return ErrTerminateRequest
session.write_response_header(...).await?;
session.write_response_body(Some(Bytes::from("Forbidden")), true).await?;
return Ok(PluginRunningResult::ErrTerminateRequest);

// Wrong: return ErrTerminateRequest without writing — triggers runtime fallback
return Ok(PluginRunningResult::ErrTerminateRequest);
```

If the plugin cannot write a response at its current stage (e.g. a sync hook
where `await` is unavailable), use `ErrResponse { status, body }` instead —
the runtime writes the response on the plugin's behalf.

**Runtime fail-safe**: a plugin that violates this convention receives a 502
Bad Gateway fallback from the runtime, and `PluginTerminateWithoutResponse`
appears in the access log `errors[]` field. Fix the plugin so it writes its
own response and the marker disappears.

## 项目特有规则

### 没有 Consumer 模型

不要按 Kong / APISIX 的“Consumer -> Plugin”思路设计。

当前仓库里：

- 凭据主要放在 Kubernetes Secret
- 插件直接引用 Secret 或解析后的 runtime 字段
- 上游身份通常通过请求头或 ctx 变量传递

### 代码注释和日志用英文

这是当前仓库的通用约束，保持和现有插件一致。

## 测试建议

优先补这四层：

1. 配置与 `preparse()` 单测
2. `MockPluginSession` 驱动的插件单测
3. 如果有 Secret 解析，补 `EdgionPluginsHandler` 侧单测
4. 通过 `ExtensionRef` 或真实路由命中的集成测试

当前 `PluginSession` 已带 `automock`，所以单测可以直接用：

- `MockPluginSession`

### 如果插件支持热更新（`partial_update` / `full_set`）

光验初始状态不够——补一个 dynamic scenario 把"YAML 更新→ 运行时行为变"的链路也跑回归。模板：移除一条凭据（BasicAuth）、切换一个枚举字段（RateLimit `onMissingKey`）。

- 直接看现有 scenario 入门：`examples/code/client/dynamic/scenarios/basic_auth_dynamic.rs` / `rate_limit_dynamic.rs`
- conf 目录约定：`examples/test/conf/EdgionPlugins/<Plugin>/DynamicTest/{initial,updates}/`（`load_conf.sh` 自动跳过 `updates/` 与 `delete/`）
- 详细新增步骤、CLI 入口、注册位置：[../../05-testing/01-integration-testing.md「动态（热更新）测试」节](../../05-testing/01-integration-testing.md)

> **避坑**：BasicAuth bcrypt LRU、RateLimit sliding window counter 等带状态缓存的字段，热更新可能 false-pass（旧 cache 命中）。优先验"整条 ref 移除"或"枚举字段切换"这种**逻辑分支**变化，不要光验"修改密码内容"或"改 rate 数值"。

另外可以参考：

- `src/types/resources/edgion_plugins/tests.rs`
- 各插件自身的 `plugin.rs` / `tests.rs`
- [../../05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md)

## 审查清单

- 插件阶段选得是否正确
- `EdgionPlugin` enum、runtime 构造、module export 是否都接齐
- 是否误用了旧版 `runtime.rs` 文档路径，而没有落到当前的 `plugin_runtime.rs`
- 如果依赖 Secret，`EdgionPluginsHandler.parse()` 和 `on_delete()` 是否补齐
- 是否正确使用 `PluginSession` / `ctx` 变量，而不是绕过抽象
- `PluginLog` 是否简洁，是否避免把大段调试信息写进 access log
- 是否覆盖成功、失败、skip/run 条件和组合执行场景

## 相关

- [docs/zh-CN/dev-guide/http-plugin-development.md](../../../docs/zh-CN/dev-guide/http-plugin-development.md)
- [observability/00-access-log.md](../../03-coding/observability/00-access-log.md)
- [13-stream-plugin-dev.md](13-stream-plugin-dev.md)
- [14-link-sys-dev.md](14-link-sys-dev.md)
- [../../05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md)
