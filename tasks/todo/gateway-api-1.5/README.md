# Gateway API v1.4 → v1.5 升级

## 概述

将项目 Gateway API 支持从 v1.4.0 升级到 v1.5.0。

涉及范围：Rust 类型定义、资源处理器、路由管理模块、CRD 文件、测试 YAML、Go conformance 模块。

## 任务清单

### P0 — 必须完成

- [ ] **CRD 文件替换** (~0.5天)
  - 替换 `config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml` (已完成)
  - 替换 `config/crd/gateway-api/gateway-api-experimental-v1.5.0.yaml` (已完成)

- [x] **TLSRoute v1alpha2 → v1** (~2-3天)
  - `src/types/resources/tls_route.rs` — apiVersion 更新
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs` — 处理器适配
  - `src/core/gateway/routes/tls/` — 路由管理逻辑适配
  - 所有 TLSRoute 测试 YAML 文件 apiVersion 从 `v1alpha2` → `v1` ✅
  - 注意：v1alpha2 将在 v1.6 中移除

### P1 — 应该完成

- [ ] **Go Conformance 模块升级** (~1-2天)
  - `examples/gateway-api-conformance/go.mod` 中 `sigs.k8s.io/gateway-api` v1.4.0 → v1.5.0 ✅
  - 适配新增的 conformance 测试用例

- [ ] **HTTP Redirect 新增状态码支持** (~0.5天)
  - 支持 303、307、308 状态码
  - 涉及 `src/types/resources/http_route.rs` 和对应路由处理逻辑

- [ ] **Gateway ResolvedRefs 状态条件** (~1天)
  - Gateway status 新增 ResolvedRefs condition 上报
  - 涉及 `src/types/resources/gateway.rs` 和状态上报逻辑

- [ ] **BackendTLSPolicy targetRef 限制检查** (~0.5天)
  - 确认实现是否已符合单 targetRef conformance 要求

- [ ] **CORS Origin CEL 验证收紧** (~0.5天)
  - 检查实验性 CORS 字段是否有不合规值

### P2 — 可选

- [ ] **ListenerSet 支持** (~3-5天)
  - 新增 ListenerSet 类型定义和资源处理器
  - 已升级到 v1 (Standard channel)，可作为新功能单独规划

## 前置条件

- 确认项目最低支持的 Kubernetes 版本 — TLSRoute v1 的 CEL 验证要求 **K8s 1.31+**
- 实验性 CRD 安装需使用 `kubectl apply --server-side`

## 预计总工作量

- 不含 ListenerSet：**5-8 个工作日**
- 含 ListenerSet：**8-13 个工作日**
