# Step 07: 逐步跑 Conformance 测试

> Phase 3：按复杂度递增逐步验证每个 conformance 测试。

## 测试顺序（由简到复杂）

### 7.1 BackendTLSPolicyInvalidKind
- **验证**：纯 status 检查，kind 不支持时 reason=InvalidKind
- **依赖**：Step 01（ancestorRef）、Step 04（reason 值）

### 7.2 BackendTLSPolicyInvalidCACertificateRef
- **验证**：status 检查 + 请求返回 5xx
- **依赖**：Step 01、Step 02（ConfigMap 支持）、Step 04

### 7.3 BackendTLSPolicyObservedGenerationBump
- **验证**：generation 跟踪，policy 更新后 observedGeneration 递增
- **依赖**：Step 01、Step 04

### 7.4 BackendTLSPolicy（主测试）
- **验证**：TLS re-encryption 数据面验证
- **依赖**：Step 01-04、Step 06（数据面 + 基础设施）

### 7.5 BackendTLSPolicySANValidation
- **验证**：SAN 验证数据面
- **依赖**：Step 01-04、Step 06

### 7.6 BackendTLSPolicyConflictResolution
- **验证**：冲突检测，多 policy 指向同一 Service
- **依赖**：Step 01-04（特别是 Step 03 冲突检测）

## 运行方式

```bash
# 单个测试
go test -run "TestConformance/BackendTLSPolicyInvalidKind" -v

# 或通过 conformance suite 的 filter
```

## 策略

- 每通过一个测试再进下一个
- 遇到失败先分析 log，确认是 controller status 问题还是数据面问题
- 数据面问题需要查 gateway 侧日志和 upstream TLS 握手
