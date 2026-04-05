# BackendTLSPolicy Conformance 测试

> 启用 Gateway API BackendTLSPolicy conformance 测试，修复 controller status 和 gateway 数据面的所有 gap。

## Background

BackendTLSPolicy 是 Gateway API 标准资源，控制 gateway 到后端服务的 TLS re-encryption。
当前 Edgion 已有基础实现（`backend_tls_policy.rs`），但与 conformance 测试要求存在多个关键差异。

## Gap 总览

| Gap | 严重度 | 说明 |
|-----|--------|------|
| 1 | 致命 | ancestorRef 指向 BackendTLSPolicy 自身，应指向 Gateway |
| 2 | 致命 | 不支持 ConfigMap 类型的 caCertificateRef（只支持 Secret） |
| 3 | 致命 | 缺少冲突检测（多 policy 指向同一 Service） |
| 4 | 影响 | status reason 值不精确（Invalid vs NoValidCACertificate 等） |
| 5 | 待验证 | Gateway/tls-backend 基础设施是否由 conformance suite 提供 |
| 6 | 待验证 | Gateway 数据面 backend TLS 连接是否正确工作 |

## 子任务

- [step-01](step-01-ancestor-ref-fix.md) — ancestorRef 指向 Gateway（Gap 1）
- [step-02](step-02-configmap-ca-support.md) — ✅ 支持 ConfigMap caCertificateRef（Gap 2）
- [step-03](step-03-conflict-detection.md) — ✅ 冲突检测逻辑（Gap 3）
- [step-04](step-04-status-reason-values.md) — ✅ 精确 status reason 值（Gap 4）
- [step-05](step-05-declare-features.md) — conformance_test.go 声明 feature
- [step-06](step-06-infra-verification.md) — 基础设施和数据面验证（Gap 5/6）
- [step-07](step-07-incremental-test-run.md) — 逐步跑 conformance 测试

## 风险

- ancestorRef 修复复杂度最高：需要 handler 能查到 targetRef Service -> Gateway 的关联关系
- ConfigMap 支持相对独立，可复用 `gateway.rs` 已有逻辑
- 冲突检测的 store 基础设施已有，handler 集成需注意 timing
- 数据面如果有 bug，可能需要额外修 gateway 侧代码
