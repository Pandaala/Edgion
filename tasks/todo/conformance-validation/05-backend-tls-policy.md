# 阶段 5: BackendTLSPolicy Conformance

**测试数**: 6
**预计耗时**: 1 天
**Features**: BackendTLSPolicy

## 运行命令

```bash
# BackendTLSPolicy 相关
./scripts/run.sh --supported-features "Gateway,HTTPRoute,ReferenceGrant,BackendTLSPolicy"

# 单个测试
./scripts/run.sh --run-test BackendTLSPolicy
```

## BackendTLSPolicy (6 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | BackendTLSPolicy | [ ] | |
| 2 | BackendTLSPolicyConflictResolution | [ ] | |
| 3 | BackendTLSPolicyInvalidCACertificateRef | [ ] | |
| 4 | BackendTLSPolicyInvalidKind | [ ] | |
| 5 | BackendTLSPolicyObservedGenerationBump | [ ] | |
| 6 | BackendTLSPolicySANValidation | [ ] | |

## 已知状态

Edgion 已实现 BackendTLSPolicy，但未跑过官方 conformance test。
主要风险在 status condition 和 CA certificate ref 验证逻辑。
