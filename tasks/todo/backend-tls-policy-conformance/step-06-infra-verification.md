# Step 06: 基础设施和数据面验证

> **Gap 5/6（待验证）**：确认 conformance suite 提供的基础设施和 gateway 数据面 TLS 连接。

## 需要确认的基础设施

BackendTLSPolicy 主测试需要：

1. **Gateway `same-namespace-with-https-listener`** — HTTPS listener 的 Gateway
2. **`tls-backend` 应用 pods** — 支持 TLS 的后端服务
3. **`tls-checks-ca-certificate` ConfigMap** — CA 证书（conformance suite 可能动态生成）

## 确认步骤

1. 查看 conformance suite 的 base resources / manifests，确认哪些资源由 suite 提供
2. 查看 Edgion 已有的 conformance setup 代码，确认是否需要额外的 setup 脚本
3. 如果 suite 不提供某些资源，需要在 Edgion 的 conformance setup 中补充

## 数据面验证（Gap 6）

`pg_upstream_peer.rs` 需要正确使用 BackendTLSPolicy 配置建立到后端的 TLS 连接：

1. 确认 gateway 侧是否正确读取 BackendTLSPolicy 的 CA 证书配置
2. 确认 TLS re-encryption（gateway -> backend）连接是否正确建立
3. 确认 SAN 验证是否按 `hostname` 字段执行

## 验证

- 部署 conformance suite 的基础资源
- 手动或通过 `BackendTLSPolicy` 主测试验证 TLS re-encryption 数据路径
