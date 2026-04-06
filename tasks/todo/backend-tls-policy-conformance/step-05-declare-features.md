# Step 05: conformance_test.go 声明 feature ✅

> Phase 2：在 conformance 测试入口声明 BackendTLSPolicy 相关 feature。

## 改动

`examples/gateway-api-conformance/conformance_test.go` 的 `declaredSupportedFeatures()` 中：

```go
// BackendTLSPolicy: CA certificate validation for backend TLS re-encryption.
supported.Insert(features.SupportBackendTLSPolicy)
supported.Insert(features.SupportBackendTLSPolicySANValidation)
```

- `SupportBackendTLSPolicy` — 已在之前添加（line 87）
- `SupportBackendTLSPolicySANValidation` — 本次新增

## 验证

- `go build ./...` — 通过
