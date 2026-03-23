# Gateway API v1.4 → v1.5 升级

## 概述

将项目 Gateway API 支持从 v1.4.0 升级到 v1.5.0。

涉及范围：Rust 类型定义、资源处理器、路由管理模块、CRD 文件、测试 YAML、Go conformance 模块。

## 任务清单

### P0 — 必须完成

- [x] **CRD 文件替换**
  - 替换 `config/crd/gateway-api/gateway-api-standard-v1.5.0.yaml` ✅
  - 替换 `config/crd/gateway-api/gateway-api-experimental-v1.5.0.yaml` ✅
  - 删除旧版 v1.4.0 CRD 文件 ✅
  - `deploy/kubernetes/scripts/install_crds.sh` 脚本已同步更新 ✅

- [x] **TLSRoute v1alpha2 → v1**
  - `src/types/resources/tls_route.rs` — apiVersion 更新 ✅
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/` — 处理器适配 ✅
  - `src/core/gateway/routes/tls/` — 路由管理逻辑适配 ✅
  - 所有 TLSRoute 测试 YAML 文件 apiVersion 从 `v1alpha2` → `v1` ✅
  - 注意：v1alpha2 将在 v1.6 中移除

### P1 — 应该完成

- [x] **HTTP Redirect 新增状态码支持（303/307/308）**
  - 代码无需改动 — `request_redirect.rs` 使用 `StatusCode::from_u16()` 转换，天然支持所有标准状态码，未做白名单限制
  - 测试 YAML 已创建：`examples/test/conf/HTTPRoute/Filters/Redirect/EdgionPlugins_default_redirect-{303,307,308}.yaml` ✅

- [ ] **Go Conformance 模块升级**
  - `examples/gateway-api-conformance/go.mod` 已将 `sigs.k8s.io/gateway-api` 升到 v1.5.0 ✅
  - 待完成：适配新增的 conformance 测试用例

- [ ] **Gateway ResolvedRefs 状态条件**
  - Gateway status 新增 ResolvedRefs condition 上报
  - 涉及 `src/types/resources/gateway.rs` 和状态上报逻辑

- [ ] **BackendTLSPolicy targetRef 限制检查**
  - 确认实现是否已符合单 targetRef conformance 要求
  - 涉及 `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs`

- [ ] **CORS Origin CEL 验证收紧**
  - 检查实验性 CORS 字段是否有不合规值
  - 涉及 `src/core/gateway/plugins/runtime/gateway_api_filters/cors_filter.rs`

### P2 — 可选

- [ ] **ListenerSet 支持**
  - 新增 ListenerSet 类型定义和资源处理器
  - 已升级到 v1 (Standard channel)，可作为新功能单独规划

## 前置条件

- 确认项目最低支持的 Kubernetes 版本 — TLSRoute v1 的 CEL 验证要求 **K8s 1.31+**
- 实验性 CRD 安装需使用 `kubectl apply --server-side`

## 当前进度（2026-03-22）

**已完成：** P0 全部完成，P1 中 HTTP Redirect 已完成（代码天然兼容，测试配置已就位）

**进行中：** Go Conformance 模块 go.mod 已升级，需适配新测试用例

**未开始：** Gateway ResolvedRefs、BackendTLSPolicy targetRef 检查、CORS 验证收紧、ListenerSet
