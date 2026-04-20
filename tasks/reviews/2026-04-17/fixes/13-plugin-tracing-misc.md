# 13 — 其他插件热路径 tracing 清理（CtxSet / RateLimit Redis / DSL / Direct / session_adapter 等）

**等级：** P1（部分 P2）
**修复类型：** 删除 tracing + 补 metrics
**工作量：** M
**溯源：** P1-Plugin-01（其余子集）+ 专项 A 新发现

## 位置

| file:line | 说明 |
|-----------|------|
| `src/core/gateway/plugins/http/ctx_set/plugin.rs:118,134,230,235` | `run_request` 内 debug，**L230 把最终 ctx 值拼进 log（敏感）** |
| `src/core/gateway/plugins/http/direct_endpoint/plugin.rs:55` | `resolve_endpoint` 每请求 warn |
| `src/core/gateway/plugins/http/request_restriction/plugin.rs:165` | 每请求 config invalid `tracing::error!` |
| `src/core/gateway/plugins/http/rate_limit_redis/plugin.rs:193` | Redis NOSCRIPT fallback 每请求 debug |
| `src/core/gateway/plugins/http/rate_limit_redis/plugin.rs:495` | 每请求 config invalid `tracing::error!` |
| `src/core/gateway/plugins/http/dsl/lang/builtins.rs:35,676,682` | DSL VM 运行 warn/error（用户可构造脚本无限触发） |
| `src/core/gateway/plugins/http/dsl/lang/vm.rs:676,703,746,751` | 同上 |
| `src/core/gateway/plugins/runtime/session_adapter.rs:619` | Webhook KeyGet 在 sync 条件评估每请求 warn（配置漏检导致可利用） |
| `src/core/gateway/plugins/runtime/gateway_api_filters/request_redirect.rs:147,153,161` | redirect 失败每请求 `tracing::error!` |
| `src/core/gateway/plugins/runtime/gateway_api_filters/debug_access_log.rs:31` | `tracing::error!` |
| `src/core/gateway/plugins/http/all_endpoint_status/plugin.rs:463,701` | wall timeout / JSON 序列化失败 warn（P2 级） |
| `src/core/gateway/plugins/runtime/request_mirror/mirror_log.rs:27` | serialize 失败 per-mirror warn（P2 级） |

**合计约 17 处。**

## 问题

各种"config invalid"/"序列化失败"/"降级路径" tracing 泛滥。`ctx_set/plugin.rs:230` 尤其严重——把 ctx 值直接拼到 log，攻击者配合恶意请求可泄漏 header/cookie 敏感值。

配置无效类问题应在 preparse 阶段拒绝，而非 run_request 打 tracing。

## 修复步骤

1. **`ctx_set/plugin.rs:230` 立即删除**（敏感值泄漏）
2. 其他 ctx_set 的 3 处 debug 也删
3. DSL VM 的 6 处 tracing 全删；改 VM 内部错误计数器（`counter!("edgion_dsl_vm_errors_total", "kind" => "runtime_panic|type_error|...")`）
4. `session_adapter.rs:619` 删除；Webhook KeyGet 在 sync 阶段调用应在 preparse 阶段拒绝配置
5. `request_restriction:165` / `rate_limit_redis:495` 的"config invalid"删 tracing，改 PluginLog `"err:config; "` + counter `edgion_plugin_config_invalid_fail_close_total{plugin=...}`
6. `request_redirect.rs:147,153,161` 删 tracing，改 PluginLog + counter
7. `debug_access_log.rs:31` 删除
8. P2 级 all_endpoint_status / mirror_log 也一并处理

## 验收

- [ ] `cargo check --all-targets` 通过
- [ ] 上述各文件 `tracing::*` 热路径全部清除
- [ ] 单测：ctx_set 运行 → 日志不含 ctx 值
- [ ] `/metrics` 可见 `edgion_plugin_config_invalid_fail_close_total`、`edgion_dsl_vm_errors_total`
- [ ] 压测：DSL 恶意脚本不产生日志洪水
