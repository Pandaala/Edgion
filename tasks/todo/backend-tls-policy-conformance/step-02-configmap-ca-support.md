# Step 02: 支持 ConfigMap caCertificateRef ✅

> **Gap 2（致命）**：所有 conformance 测试使用 `kind: ConfigMap`，当前只接受 `kind: Secret`。
>
> **状态**: completed (2026-04-05)

## 现状

`backend_tls_policy.rs` 第 282-283 行：

```rust
is_core_api_group(&ca_ref.group) && ca_ref.kind == "Secret"
```

对 ConfigMap 直接 skip 并报错。

## Conformance 要求

所有 conformance 测试的 BackendTLSPolicy 都使用：

```yaml
caCertificateRefs:
- kind: ConfigMap
  name: tls-checks-ca-certificate
```

## 修复方案

1. 在 `is_secret_ref()` 旁添加 `is_configmap_ref()` 检查
2. 当 `kind == "ConfigMap"` 时，使用 `get_configmap` 加载 ConfigMap
3. 从 ConfigMap 的 `ca.crt` key 解析 CA 证书数据
4. 参考 `gateway.rs` 第 1196-1225 行已有的 ConfigMap CA 解析逻辑

## 参考实现

`gateway.rs` 中已有完整的 ConfigMap CA 加载逻辑，可以提取为共享函数或直接复用模式。

## 验证

- `BackendTLSPolicyInvalidCACertificateRef` 测试检查无效 ConfigMap ref 的 status
- `BackendTLSPolicy` 主测试使用有效 ConfigMap ref 做 TLS re-encryption

## 完成记录

### 改动文件

| 文件 | 改动 |
|------|------|
| `src/.../handlers/backend_tls_policy.rs` | `parse_policy()` 新增 ConfigMap 分支；`update_policy_status()` 统一校验；��增 `is_configmap_ref()`；`is_ca_secret_resolved` → `is_ca_resolved` |
| `src/.../file_system/controller.rs` | Phase 1 注册 ConfigMapHandler |
| `src/.../api/namespaced_handlers.rs` | create/update API 新增 ConfigMap 分支 |
| `examples/test/conf/Status/BackendTLSPolicy/ConfigMap_edgion-test_test-ca-configmap.yaml` | 新增：测试 ConfigMap |
| `examples/test/conf/Status/BackendTLSPolicy/BackendTLSPolicy_edgion-test_configmap-ca.yaml` | 新增：引用有效 ConfigMap 的 policy |
| `examples/test/conf/Status/BackendTLSPolicy/BackendTLSPolicy_edgion-test_configmap-ca-missing.yaml` | 新增：引用不存在 ConfigMap 的 policy |
| `examples/code/client/suites/status/backend_tls/status_test.rs` | 新增 3 个测试用例 |

### 集成测试结果

7/7 passed（含新增 3 个 ConfigMap 测试用例）
