# TLSRoute Terminate 模式一致性测试修复

## 优先级: P1
## 工作量: 中（预计 2-3 天）
## 类型: 一致性修复

---

## 问题描述

TLSRoute Terminate 模式的数据面路径已存在（`src/core/gateway/routes/tls/proxy.rs`），
但 Gateway API 一致性测试尚未系统性通过。这是一个 Extended 级别特性（`TLSRouteModeTerminate`）。

Terminate 模式下，Gateway 终止 TLS 连接，根据 SNI 路由到后端，将解密后的 TCP 流量转发给后端。

## 当前状态

### 已有
- Passthrough 模式：✅ 完整实现，一致性测试通过
- Terminate 模式数据面：存在于 `proxy.rs`
- SNI 匹配逻辑：✅ 通配符支持
- Listener TLS Terminate 配置解析：✅

### 缺失/待验证
- 一致性测试未系统性验证
- 可能存在的边界情况（证书选择、SNI 不匹配处理等）
- 与 Passthrough 模式在同一端口上的区分逻辑

## 涉及文件

1. `src/core/gateway/routes/tls/proxy.rs` — Terminate 模式代理实现
2. `src/core/gateway/routes/tls/passthrough_proxy.rs` — Passthrough 模式（参考）
3. `src/core/gateway/routes/tls/gateway_tls_routes.rs` — TLS 路由匹配
4. `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` — TLS 证书配置
5. `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs` — Controller
6. `examples/test/conf/TLSRoute/` — 集成测试
7. `examples/gateway-api-conformance/` — 一致性测试入口

## 实现建议

### 调试步骤
1. 运行 TLSRoute 相关一致性测试，收集失败用例
2. 对比 Passthrough 模式的成功路径，找出 Terminate 模式的差异
3. 检查以下常见问题：
   - Listener 是否正确识别 `mode: Terminate` 并配置 TLS 终止
   - 证书选择逻辑是否按 SNI 正确匹配
   - 解密后的 TCP 流量是否正确转发到后端
   - 后端连接是否需要重新加密（取决于 BackendTLSPolicy）

### 关键测试场景
- 基本 Terminate：TLS 终止 + SNI 路由 + 明文转发到后端
- 多 SNI Terminate：不同域名路由到不同后端
- 通配符 SNI：`*.example.com` 匹配
- 无匹配 SNI：连接拒绝或关闭

## 参考 Skills

- `skills/08-kubernetes/01-gateway-api/SKILL.md` — Gateway API 规范理解
- `skills/09-misc/debugging-tls-gateway.md` — TLS 网关调试
- `skills/05-testing/00-integration-testing.md` — 集成测试
- `skills/01-architecture/02-gateway/SKILL.md` — Gateway 数据面架构

## 相关已有任务

- `tasks/todo/missing-features/02-tlsroute-terminate.md` — 可能有更详细的实现计划

## 验证标准

- TLSRoute Terminate 一致性测试全部通过
- Passthrough 模式不受影响（回归测试）
- 集成测试覆盖基本 Terminate、多 SNI、通配符场景
- `cargo check --all-targets` 通过
