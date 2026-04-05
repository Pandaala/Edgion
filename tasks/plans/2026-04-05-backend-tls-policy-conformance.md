# BackendTLSPolicy Conformance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pass all 6 Gateway API BackendTLSPolicy conformance tests by fixing ancestorRef resolution, status reason values, and adding Route→BTP requeue chain.

**Architecture:** BackendTLSPolicy targets Services, but conformance requires `status.ancestors[].ancestorRef` to point to Gateways. We resolve this via a 3-hop lookup chain: `BTP.targetRef(Service) → ServiceRefManager → Routes → GatewayRouteIndex → Gateways`. Reactivity is achieved by adding BTP requeue in Route on_change/on_delete, using the existing `BackendTLSPolicyTargetManager` to find BTPs bound to backend Services.

**Tech Stack:** Rust, Gateway API v1.5, existing `BidirectionalRefManager` / `GatewayRouteIndex` infrastructure.

**Pre-conditions (already done):** Step-02 (ConfigMap CA) ✅, Step-03 (conflict detection) ✅.

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/gateway_route_index.rs` | Modify | Add `get_gateways_for_route()` reverse lookup method |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs` | Modify | Add shared `requeue_backend_tls_policies_for_route()` utility |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs` | Modify | Rewrite `update_policy_status()` to resolve real Gateway ancestors; fix reason values |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs` | Modify | Add BTP-specific reason constants |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs` | Modify | Add BTP requeue in on_change/on_delete |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs` | Modify | Add BTP requeue in on_change/on_delete |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs` | Modify | Add BTP requeue in on_change/on_delete |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tcp_route.rs` | Modify | Add BTP requeue in on_change/on_delete |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/udp_route.rs` | Modify | Add BTP requeue in on_change/on_delete |
| `examples/gateway-api-conformance/conformance_test.go` | Modify | Declare `SupportBackendTLSPolicy` feature |

---

## Task 1: GatewayRouteIndex 反向查询

**Files:**
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/gateway_route_index.rs`

GatewayRouteIndex 已有反向索引 `RouteEntry → Set<gateway_key>`，但没有公开查询方法。添加一个。

- [ ] **Step 1: 添加 `get_gateways_for_route` 方法**

在 `impl GatewayRouteIndex` 块中，`get_routes_for_gateway` 方法（第 174 行）之后添加：

```rust
/// Get all Gateways that a given route references via parentRefs.
///
/// Returns gateway keys (e.g., `"default/my-gateway"`).
/// Used by BackendTLSPolicy to resolve ancestors:
/// BTP.targetRef(Service) → ServiceRefManager → Route → this method → Gateway(s)
pub fn get_gateways_for_route(&self, route_kind: ResourceKind, route_key: &str) -> Vec<String> {
    let entry = RouteEntry {
        kind: route_kind,
        key: route_key.to_string(),
    };
    let rev = self.reverse.read().unwrap();
    match rev.get(&entry) {
        Some(gw_keys) => gw_keys.iter().cloned().collect(),
        None => Vec::new(),
    }
}
```

- [ ] **Step 2: 添加单元测试**

在同文件的 `mod tests` 块末尾（`test_non_gateway_parent_ref_ignored` 测试之后）添加：

```rust
#[test]
fn test_get_gateways_for_route() {
    let idx = GatewayRouteIndex::new();
    idx.update_route(
        ResourceKind::HTTPRoute,
        "default/r1",
        &[pr("default", "gw1"), pr("other", "gw2")],
        "default",
    );

    let mut gateways = idx.get_gateways_for_route(ResourceKind::HTTPRoute, "default/r1");
    gateways.sort();
    assert_eq!(gateways, vec!["default/gw1", "other/gw2"]);

    // Non-existent route returns empty
    let empty = idx.get_gateways_for_route(ResourceKind::HTTPRoute, "default/nonexistent");
    assert!(empty.is_empty());
}
```

- [ ] **Step 3: 运行测试验证**

Run: `cargo test --lib gateway_route_index -- --nocapture`
Expected: 所有测试通过，包括新增的 `test_get_gateways_for_route`

---

## Task 2: Status Reason 常量

**Files:**
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs`

添加 BackendTLSPolicy conformance 要求的精确 reason 值。

- [ ] **Step 1: 在 `condition_reasons` module 中添加常量**

在 `status_utils.rs` 的 `condition_reasons` module 中，`POLICY_CONFLICTED` 常量（第 65 行）之后添加：

```rust
// BackendTLSPolicy-specific reasons (Gateway API v1.5 conformance)
/// CACertificateRef references an unknown or unsupported resource kind.
pub const INVALID_KIND: &str = "InvalidKind";
/// CACertificateRef cannot be resolved (resource not found or misconfigured).
pub const INVALID_CA_CERTIFICATE_REF: &str = "InvalidCACertificateRef";
```

- [ ] **Step 2: 运行编译检查**

Run: `cargo check --all-targets`
Expected: 编译通过

---

## Task 3: 重写 ancestorRef 解析 + reason 值修复

**Files:**
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs:234-384`

这是核心改动。重写 `update_policy_status` 方法，实现：
1. 通过 ServiceRefManager + GatewayRouteIndex 三步查询解析真实 Gateway
2. 为每个 Gateway 生成独立的 ancestor entry
3. 使用 conformance 精确 reason 值

- [ ] **Step 1: 添加 resolve_ancestor_gateways 辅助方法**

在 `impl BackendTlsPolicyHandler` 块中（`is_configmap_ref` 方法之前，约第 408 行前）添加：

```rust
/// Resolve the Gateway ancestors for a BackendTLSPolicy.
///
/// Chain: BTP.targetRef(Service) → ServiceRefManager(service→routes) → GatewayRouteIndex(route→gateways)
///
/// Returns deduplicated list of `(gateway_ns, gateway_name)`.
/// Empty if no route references the target service or no route is attached to a gateway.
fn resolve_ancestor_gateways(btp: &BackendTLSPolicy) -> Vec<(String, String)> {
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::gateway_route_index::get_gateway_route_index;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::service_ref::get_service_ref_manager;

    let policy_ns = btp.metadata.namespace.as_deref().unwrap_or("default");
    let service_ref_manager = get_service_ref_manager();
    let route_index = get_gateway_route_index();

    let mut gateways = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for target_ref in &btp.spec.target_refs {
        let target_ns = target_ref.namespace.as_deref().unwrap_or(policy_ns);
        let service_key = format!("{}/{}", target_ns, target_ref.name);

        // Step 1: Service → Routes
        let routes = service_ref_manager.get_refs(&service_key);

        for route_ref in &routes {
            let route_key = route_ref.resource_key();

            // Step 2: Route → Gateways
            let gw_keys = route_index.get_gateways_for_route(route_ref.kind, &route_key);

            for gw_key in gw_keys {
                if seen.insert(gw_key.clone()) {
                    // Parse "ns/name" format
                    if let Some((ns, name)) = gw_key.split_once('/') {
                        gateways.push((ns.to_string(), name.to_string()));
                    }
                }
            }
        }
    }

    gateways
}
```

- [ ] **Step 2: 重写 update_policy_status 方法**

替换 `update_policy_status` 方法的完整实现（第 234-384 行）：

```rust
fn update_policy_status(&self, btp: &mut BackendTLSPolicy, _ctx: &HandlerContext, validation_errors: &[String]) {
    let generation = btp.metadata.generation;

    // --- Classify CA reference errors with precise reasons ---
    let mut kind_errors = Vec::new();      // InvalidKind
    let mut ref_errors = Vec::new();       // InvalidCACertificateRef
    let mut ca_ref_count: usize = 0;
    let mut ca_invalid_count: usize = 0;

    if !btp.spec.use_system_ca {
        if let Some(refs) = &btp.spec.validation.ca_certificate_refs {
            ca_ref_count = refs.len();
            for ca_ref in refs {
                if Self::is_secret_ref(ca_ref) || Self::is_configmap_ref(ca_ref) {
                    if !Self::is_ca_resolved(btp, &ca_ref.name) {
                        ref_errors.push(format!(
                            "{} '{}' not found or missing '{}' key",
                            ca_ref.kind, ca_ref.name, CA_CERT
                        ));
                        ca_invalid_count += 1;
                    }
                } else {
                    kind_errors.push(format!(
                        "Unsupported caCertificateRef kind: {}/{}",
                        ca_ref.group, ca_ref.kind
                    ));
                    ca_invalid_count += 1;
                }
            }
        }
    }

    // Client certificate errors
    let mut client_cert_errors = Vec::new();
    match btp.client_certificate_secret_ref() {
        Ok(Some(client_ref)) => {
            if btp.spec.resolved_client_certificate.is_none() {
                client_cert_errors.push(format!("Client certificate Secret '{}' not found", client_ref.name));
            } else if let Some(secret) = &btp.spec.resolved_client_certificate {
                if !Self::secret_has_client_cert_pair(secret) {
                    client_cert_errors.push(format!(
                        "Client certificate Secret '{}' must contain '{}' and '{}'",
                        client_ref.name, CERT, KEY
                    ));
                }
            }
        }
        Ok(None) => {}
        Err(err) => client_cert_errors.push(err),
    }

    // --- Resolve ancestor Gateways ---
    let ancestor_gateways = Self::resolve_ancestor_gateways(btp);

    let status = btp
        .status
        .get_or_insert_with(|| BackendTLSPolicyStatus { ancestors: vec![] });

    // If no Gateways found, create a single "unknown" ancestor so status is still reported.
    // This handles the timing case where BTP processes before Routes.
    let gateway_refs: Vec<ParentReference> = if ancestor_gateways.is_empty() {
        vec![ParentReference {
            group: Some("gateway.networking.k8s.io".to_string()),
            kind: Some("Gateway".to_string()),
            namespace: btp.metadata.namespace.clone(),
            name: String::new(),
            section_name: None,
            port: None,
        }]
    } else {
        ancestor_gateways
            .iter()
            .map(|(ns, name)| ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some(ns.clone()),
                name: name.clone(),
                section_name: None,
                port: None,
            })
            .collect()
    };

    // Rebuild ancestors list
    status.ancestors.clear();

    // --- Conflict detection ---
    let is_conflicted = if validation_errors.is_empty() {
        let policy_ns = btp.metadata.namespace.as_deref().unwrap_or("default");
        let policy_key = format!("{}/{}", policy_ns, btp.metadata.name.as_deref().unwrap_or(""));
        btp.spec.target_refs.iter().any(|target_ref| {
            let target_key =
                make_target_conflict_key(policy_ns, &target_ref.name, target_ref.section_name.as_deref());
            !is_policy_winner(&policy_key, &target_key)
        })
    } else {
        false
    };

    for gw_ref in &gateway_refs {
        let mut conditions = Vec::new();

        // Accepted condition
        if !validation_errors.is_empty() {
            update_condition(
                &mut conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    "Invalid",
                    validation_errors.join("; "),
                    generation,
                ),
            );
        } else if is_conflicted {
            update_condition(
                &mut conditions,
                condition_false(
                    condition_types::ACCEPTED,
                    condition_reasons::POLICY_CONFLICTED,
                    "Policy conflicts with an older policy targeting the same backend",
                    generation,
                ),
            );
        } else {
            update_condition(
                &mut conditions,
                condition_true(
                    condition_types::ACCEPTED,
                    condition_reasons::ACCEPTED,
                    "Policy accepted",
                    generation,
                ),
            );
        }

        // ResolvedRefs condition
        let has_kind_errors = !kind_errors.is_empty();
        let has_ref_errors = !ref_errors.is_empty();
        let has_client_errors = !client_cert_errors.is_empty();

        if has_kind_errors {
            update_condition(
                &mut conditions,
                condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::INVALID_KIND,
                    kind_errors.join("; "),
                    generation,
                ),
            );
        } else if has_ref_errors || has_client_errors {
            let all_errors: Vec<String> = ref_errors.iter().chain(client_cert_errors.iter()).cloned().collect();
            update_condition(
                &mut conditions,
                condition_false(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::INVALID_CA_CERTIFICATE_REF,
                    all_errors.join("; "),
                    generation,
                ),
            );
        } else {
            update_condition(
                &mut conditions,
                condition_true(
                    condition_types::RESOLVED_REFS,
                    condition_reasons::RESOLVED_REFS,
                    "All references resolved",
                    generation,
                ),
            );
        }

        status.ancestors.push(PolicyAncestorStatus {
            ancestor_ref: gw_ref.clone(),
            controller_name: self.controller_name.clone(),
            conditions,
        });
    }
}
```

- [ ] **Step 3: 运行编译和测试**

Run: `cargo check --all-targets && cargo test --all`
Expected: 编译通过，所有既有测试通过

---

## Task 4: Route → BTP Requeue 工具函数

**Files:**
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs`

在 Route on_change/on_delete 中复用的共享函数。通过 ServiceRefManager 反向索引拿到 Route 依赖的 Service 列表，再通过 BackendTLSPolicyTargetManager 找到绑定的 BTP 并 requeue。

- [ ] **Step 1: 添加 `requeue_backend_tls_policies_for_route` 函数**

在 `handlers/mod.rs` 中 `remove_from_gateway_route_index` 函数（约第 102-105 行）之后添加：

```rust
/// Requeue BackendTLSPolicies bound to backend Services of this route.
///
/// When a route changes, any BTP targeting its backend Services needs to
/// re-resolve its ancestor Gateways (the route may have changed parentRefs).
///
/// Chain: Route → ServiceRefManager(reverse: route→services) → BackendTLSPolicyTargetManager(forward: service→BTPs)
pub(crate) async fn requeue_backend_tls_policies_for_route(
    route_kind: ResourceKind,
    route_ns: &str,
    route_name: &str,
    ctx: &HandlerContext,
) {
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::backend_tls_policy_ref::get_backend_tls_policy_target_manager;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::service_ref::get_service_ref_manager;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::ResourceRef;

    let route_ref = ResourceRef::new(route_kind, Some(route_ns.to_string()), route_name.to_string());
    let service_keys = get_service_ref_manager().get_dependencies(&route_ref.ref_key());

    if service_keys.is_empty() {
        return;
    }

    let target_manager = get_backend_tls_policy_target_manager();
    for service_key in &service_keys {
        let btps = target_manager.get_refs(service_key);
        for btp_ref in btps {
            tracing::debug!(
                route = %route_ref.resource_key(),
                btp = %btp_ref.resource_key(),
                service = %service_key,
                "Requeuing BackendTLSPolicy due to route change"
            );
            ctx.requeue(btp_ref.kind_str(), btp_ref.resource_key()).await;
        }
    }
}
```

- [ ] **Step 2: 运行编译检查**

Run: `cargo check --all-targets`
Expected: 编译通过

---

## Task 5: 所有 Route Handler 集成 BTP Requeue

**Files:**
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs`
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs`
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tcp_route.rs`
- Modify: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/udp_route.rs`

每个 Route handler 的 on_change 和 on_delete 末尾调用 `requeue_backend_tls_policies_for_route`。所有 5 个 handler 改法相同。

- [ ] **Step 1: 修改 http_route.rs**

`on_change` 方法末尾（`requeue_parent_gateways` 调用之后）添加：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::HTTPRoute, route_ns, route_name, ctx).await;
```

`on_delete` 方法中，在 `clear_service_backend_refs` 调用**之前**添加（因为 clear 后 ServiceRefManager 反向索引就没了）：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::HTTPRoute, route_ns, route_name, ctx).await;
```

确保 `requeue_backend_tls_policies_for_route` 在导入列表中（已通过 `use super::...` 或 `handlers::mod.rs` re-export 可达）。

- [ ] **Step 2: 同样修改 grpc_route.rs**

on_change 末尾添加：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::GRPCRoute, route_ns, route_name, ctx).await;
```

on_delete 中 `clear_service_backend_refs` 之前添加：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::GRPCRoute, route_ns, route_name, ctx).await;
```

- [ ] **Step 3: 同样修改 tls_route.rs**

在 `on_change_route` 方法末尾添加：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::TLSRoute, route_ns, route_name, ctx).await;
```

在 `on_delete_route` 方法中 `clear_service_backend_refs` 之前添加：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::TLSRoute, route_ns, route_name, ctx).await;
```

- [ ] **Step 4: 同样修改 tcp_route.rs**

on_change 末尾：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::TCPRoute, route_ns, route_name, ctx).await;
```

on_delete 中 `clear_service_backend_refs` 之前：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::TCPRoute, route_ns, route_name, ctx).await;
```

- [ ] **Step 5: 同样修改 udp_route.rs**

on_change 末尾：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::UDPRoute, route_ns, route_name, ctx).await;
```

on_delete 中 `clear_service_backend_refs` 之前：
```rust
requeue_backend_tls_policies_for_route(ResourceKind::UDPRoute, route_ns, route_name, ctx).await;
```

- [ ] **Step 6: 运行完整检查**

Run: `cargo check --all-targets && cargo clippy --all-targets && cargo test --all`
Expected: 编译通过，无 clippy warnings，所有测试通过

---

## Task 6: 声明 Conformance Feature

**Files:**
- Modify: `examples/gateway-api-conformance/conformance_test.go:27-90`

- [ ] **Step 1: 添加 feature 声明**

在 `conformance_test.go` 的 `declaredSupportedFeatures()` 函数中，`SupportGRPCRoute` 之前添加：

```go
// BackendTLSPolicy: CA certificate validation + SAN match for backend TLS re-encryption
supported.Insert(features.SupportBackendTLSPolicy)
```

注意：暂不声明 `SupportBackendTLSPolicySANValidation`，等基础 BTP 测试全部通过后再加。

- [ ] **Step 2: 运行编译检查**

Run: `cd examples/gateway-api-conformance && go build ./...`
Expected: 编译通过

---

## Task 7: 更新文档

**Files:**
- Modify: `skills/08-kubernetes/01-gateway-api/01-not-implemented.md`
- Modify: `skills/08-kubernetes/01-gateway-api/SKILL.md`
- Modify: `skills/01-architecture/05-resources/12-backend-tls-policy.md`
- Modify: `skills/01-architecture/05-resources/19-resource-dependency.md`
- Modify: `tasks/references/revalidation-and-requeue-map.md`

- [ ] **Step 1: 更新 01-not-implemented.md**

将 BackendTLSPolicy Conformance 部分从"实现中"更新为"已实现"，移除待完成 Gap 列表。

- [ ] **Step 2: 更新 12-backend-tls-policy.md**

更新 `update_status` 部分：移除"待修复"标注，描述新的 ancestor 解析链路。在跨资源关联表中添加：
```
| BackendTLSPolicy ← Route on_change | Route | ServiceRefManager + BackendTLSPolicyTargetManager | Route 变更时 requeue BTP 以更新 ancestor |
```

- [ ] **Step 3: 更新 19-resource-dependency.md**

在 Store 与写入/读取方表格中添加 BackendTLSPolicyTargetManager 条目。

- [ ] **Step 4: 更新 revalidation-and-requeue-map.md**

在运行时级联 Requeue 部分添加新的 Route → BTP 路径。更新触发矩阵。

---

## 关键设计决策

### ancestor 为空时的处理

当 BTP 处理时没有 Route 引用其 target Service（init 乱序或确实无 Route），`resolve_ancestor_gateways` 返回空。此时创建一个 name 为空的占位 ancestor，确保 status 有内容。后续 Route 处理后 requeue BTP，ancestor 会被正确填充。

### on_delete 中 BTP requeue 的时序

Route on_delete 中，`requeue_backend_tls_policies_for_route` 必须在 `clear_service_backend_refs` **之前**调用，因为 clear 会删除 ServiceRefManager 中的反向索引（route → services），之后就查不到 service keys 了。

### Reason 值优先级

ResolvedRefs 条件的 reason 选择：
1. `InvalidKind` — 如果有任何 unsupported kind（优先报告）
2. `InvalidCACertificateRef` — 如果有 ref 不能解析
3. `ResolvedRefs` (True) — 全部解析成功
