# 熔断器（Circuit Breaker）

> 为 Gateway 添加 per-service/per-endpoint 被动逐出机制，防止级联故障，保护上游服务。

## Background

当前健康检查是被动剔除，没有主动熔断。上游持续失败时，请求仍会不断打过去，造成级联故障。
Kong/APISIX/Envoy 都把熔断作为核心功能，缺了会被用户直接比较。

## 已完成的前置工作

**HTTPRoute Retry-on-Status（2026-03-25 已合入）：**

- 实现了 Gateway API 标准的 `HTTPRouteRetry`（`attempts`/`backoff`/`codes`）
- 当上游返回配置的状态码（如 502/503）时，自动重试到其他后端
- 重试是 **per-route** 的即时补救，熔断器是 **per-service/per-backend** 的统计预防
- 两者互补不冲突：retry 帮当前请求找出路，circuit breaker 帮后续请求避坑
- 关键文件：`pg_upstream_response_filter.rs`、`pg_error_while_proxy.rs`、`pg_upstream_peer.rs`

## 设计决策

1. **粒度**：per-service + per-endpoint，状态跨路由共享（不是 per-HTTPRoute）
2. **配置挂载**：Service annotation `edgion.io/circuit-breaker`，不需要独立 CRD
3. **代码位置**：`src/core/gateway/backends/circuit_breaker/`（与 `health/` 同层）
4. **实现方式**：Envoy outlier_detection 精简版 — 连续失败计数 + 定时逐出 + 自动恢复
5. **失败计数 hook 点**：
   - 连接失败：`pg_fail_to_connect.rs`
   - 代理错误：`pg_error_while_proxy.rs`
   - 上游 5xx：`pg_upstream_response_filter.rs`
   - 上游 2xx-4xx：`pg_upstream_response_filter.rs`（record_success 重置计数）
6. **LB 集成点**：`backends/mod.rs` 的 `is_backend_healthy()` 扩展
7. **恢复方式**：逐出到期后自动恢复（`is_ejected()` 检查时自动清除），不需要额外探针线程

## 配置

Service annotation `edgion.io/circuit-breaker`：

```yaml
edgion.io/circuit-breaker: |
  consecutiveErrors: 5      # 连续失败 N 次后逐出（默认 5）
  ejectionSeconds: 30        # 逐出时间（默认 30s）
  maxEjectionPercent: 50     # 最大逐出比例（默认 50%，0=不限制）
```

## 实现状态

| 模块 | 内容 | 状态 |
|------|------|------|
| 数据结构 | `CircuitBreakerStore`（DashMap 无锁并发） | done |
| 配置解析 | `edgion.io/circuit-breaker` annotation 解析 + 验证 | done |
| 逐出逻辑 | 连续失败计数 → 逐出 → 定时自动恢复 | done |
| Hook 接入 | `fail_to_connect`/`error_while_proxy`/`upstream_response_filter` | done |
| LB 过滤 | `is_backend_healthy()` 加入逐出检查 | done |
| Service 同步 | `conf_handler_impl.rs` 中解析 CB annotation | done |
| 单元测试 | 13 个测试（config 6 + store 7） | done |
| 集成测试 | 3 个测试（正常/逐出/恢复） | done |
| Prometheus | `ejections_total`、`ejected_endpoints`、`rejections_total` 指标 | done |

## 关键文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/backends/circuit_breaker/config.rs` | 配置结构体 + annotation 解析 |
| `src/core/gateway/backends/circuit_breaker/store.rs` | 逐出状态存储（DashMap） |
| `src/core/gateway/backends/circuit_breaker/mod.rs` | 模块入口 |
| `src/core/gateway/backends/mod.rs` | `is_backend_healthy()` 集成 |
| `src/core/gateway/backends/discovery/services/conf_handler_impl.rs` | Service 同步时解析 CB |
| `src/core/gateway/routes/http/proxy_http/pg_fail_to_connect.rs` | 连接失败 hook |
| `src/core/gateway/routes/http/proxy_http/pg_error_while_proxy.rs` | 代理错误 hook |
| `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs` | 上游响应 hook |

## Status

- [x] 研究阶段（见 step-01-research.md）
- [x] 前置工作：HTTPRoute Retry-on-Status
- [x] 熔断器核心实现（endpoint 逐出 + 自动恢复）
- [x] 集成测试
- [x] Prometheus 指标
