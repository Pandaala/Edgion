# Step 05: conformance_test.go 声明 feature

> Phase 2：在 conformance 测试入口声明 BackendTLSPolicy 相关 feature。

## 改动

修改 `conformance_test.go`，添加 feature 声明：

```go
features.SupportBackendTLSPolicy
features.SupportBackendTLSPolicySANValidation
```

## 前置条件

- Step 01-04 的 controller 改动完成
- 确认 feature 常量名是否正确（查看 gateway-api conformance 包的 features 定义）

## 验证

- 声明后 conformance suite 会包含 BackendTLSPolicy 相关测试用例
- 可以先只声明 `SupportBackendTLSPolicy`，通过后再加 SANValidation
