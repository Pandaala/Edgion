# HTTP Plugin Development

用于这些任务：

- 新增或扩展 `EdgionPlugins` HTTP 层插件
- 调整 `PluginRuntime`、`ExtensionRef`、`PluginSession`、条件执行或 PluginLog
- 排查“插件配置有了，但路由没执行 / 执行顺序不对 / Secret 没解析 / access log 不对”

## 先读这些真实入口

- [../../docs/zh-CN/dev-guide/http-plugin-development.md](../../docs/zh-CN/dev-guide/http-plugin-development.md)
- [../../src/types/resources/edgion_plugins/mod.rs](../../src/types/resources/edgion_plugins/mod.rs)
- [../../src/types/resources/edgion_plugins/entry.rs](../../src/types/resources/edgion_plugins/entry.rs)
- [../../src/types/resources/edgion_plugins/edgion_plugin.rs](../../src/types/resources/edgion_plugins/edgion_plugin.rs)
- [../../src/core/gateway/plugins/runtime/plugin_runtime.rs](../../src/core/gateway/plugins/runtime/plugin_runtime.rs)
- [../../src/core/gateway/plugins/runtime/conditional_filter.rs](../../src/core/gateway/plugins/runtime/conditional_filter.rs)
- [../../src/core/gateway/plugins/runtime/traits/session.rs](../../src/core/gateway/plugins/runtime/traits/session.rs)
- [../../src/core/gateway/plugins/runtime/log.rs](../../src/core/gateway/plugins/runtime/log.rs)
- [../../src/core/gateway/plugins/http/mod.rs](../../src/core/gateway/plugins/http/mod.rs)
- [../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs](../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs)
- [../../src/core/gateway/plugins/http/conf_handler_impl.rs](../../src/core/gateway/plugins/http/conf_handler_impl.rs)
- [../../src/core/gateway/plugins/http/plugin_store.rs](../../src/core/gateway/plugins/http/plugin_store.rs)
- [../../src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs](../../src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs)

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

## 先判断它是不是“真正的 HTTP 插件”

它通常属于 HTTP 插件，如果需求依赖：

- Header / Cookie / Query / Path
- 认证、限流、镜像、重写
- 上游响应头
- 上游响应体 chunk
- `PluginSession` 上下文变量

如果需求只依赖 IP、监听端口、SNI、mTLS，先转去：

- [02-stream-plugin-dev.md](02-stream-plugin-dev.md)

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

- [../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs](../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs)

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

### 1. 先决定插件是 `EdgionPlugin` 还是 Gateway API filter adapter

如果是 Edgion 自定义插件：

- 走 `EdgionPlugin` enum
- 走 `EdgionPlugins` 资源与 `PluginStore`

如果是标准 Gateway API filter adapter：

- 更可能落在 `src/core/gateway/plugins/runtime/gateway_api_filters/`
- 不一定需要动 `EdgionPlugin` enum

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

- [../../src/types/resources/edgion_plugins/edgion_plugin.rs](../../src/types/resources/edgion_plugins/edgion_plugin.rs)

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

- [../../src/core/gateway/plugins/runtime/plugin_runtime.rs](../../src/core/gateway/plugins/runtime/plugin_runtime.rs)

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

- [../../src/core/gateway/plugins/runtime/conditional_filter.rs](../../src/core/gateway/plugins/runtime/conditional_filter.rs)

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

- [../../src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs](../../src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs)

当前行为是：

- 运行时从 `PluginStore` 按 `namespace/name` 取 `EdgionPlugins`
- 使用 `spec.plugin_runtime` 执行
- 通过深度限制和引用栈防循环
- 把嵌套插件日志压入 `edgion_plugins` 专用日志结构

如果你在调试“ExtensionRef 没生效 / 嵌套插件日志不见了 / 递归引用”，先从这里追。

## `PluginSession` 和常用 API

最常用的读写入口在：

- [../../src/core/gateway/plugins/runtime/traits/session.rs](../../src/core/gateway/plugins/runtime/traits/session.rs)

高频 API：

- `header_value()` / `request_headers()`
- `get_path()` / `get_query_param()` / `get_cookie()`
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

- [../../src/core/gateway/plugins/runtime/log.rs](../../src/core/gateway/plugins/runtime/log.rs)

规则保持简单：

- 记录结果，不记录长篇内部过程
- 用短语和分号结尾，例如 `OK u=jack; `
- verbose 调试用 `tracing::debug!()`，不要把 access log 当调试 dump
- 条件跳过 `cond_skip` 由 conditional wrapper 自动写，不要重复造自己的格式

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

另外可以参考：

- `src/types/resources/edgion_plugins/tests.rs`
- 各插件自身的 `plugin.rs` / `tests.rs`
- [../04-testing/01-integration-testing.md](../04-testing/01-integration-testing.md)

## 审查清单

- 插件阶段选得是否正确
- `EdgionPlugin` enum、runtime 构造、module export 是否都接齐
- 是否误用了旧版 `runtime.rs` 文档路径，而没有落到当前的 `plugin_runtime.rs`
- 如果依赖 Secret，`EdgionPluginsHandler.parse()` 和 `on_delete()` 是否补齐
- 是否正确使用 `PluginSession` / `ctx` 变量，而不是绕过抽象
- `PluginLog` 是否简洁，是否避免把大段调试信息写进 access log
- 是否覆盖成功、失败、skip/run 条件和组合执行场景

## 相关

- [../../docs/zh-CN/dev-guide/http-plugin-development.md](../../docs/zh-CN/dev-guide/http-plugin-development.md)
- [observability/00-access-log.md](../03-coding/observability/00-access-log.md)
- [02-stream-plugin-dev.md](02-stream-plugin-dev.md)
- [../04-testing/01-integration-testing.md](../04-testing/01-integration-testing.md)
