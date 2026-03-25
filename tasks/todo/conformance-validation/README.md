# Conformance Validation — 逐步验证已有功能

目标：用 Gateway API v1.5.0 官方 conformance tests 验证 Edgion 现有实现，逐模块跑测试、修 bug、打勾。

## 执行顺序

| 阶段 | 文件 | 测试数 | 预计耗时 | 前置条件 |
|------|------|--------|---------|---------|
| 1 | [01-http-core.md](01-http-core.md) | 23 | 1-2 天 | 集群就绪 |
| 2 | [02-http-extended.md](02-http-extended.md) | 29 | 2-3 天 | 阶段 1 通过 |
| 3 | [03-grpc.md](03-grpc.md) | 5 | 0.5 天 | 阶段 1 通过 |
| 4 | [04-tls-passthrough.md](04-tls-passthrough.md) | 8 | 1-2 天 | 阶段 1 通过 |
| 5 | [05-backend-tls-policy.md](05-backend-tls-policy.md) | 6 | 1 天 | 阶段 1 通过 |
| 6 | [06-gateway-extended.md](06-gateway-extended.md) | 10 | 1-2 天 | 阶段 1 通过 |

## 通用流程

每个阶段的工作流程：

1. **跑测试**：用脚本跑该阶段的测试
2. **记录结果**：在对应 md 文件中标记 pass/fail
3. **分析失败**：对每个失败的测试，定位根因
4. **修复**：修代码，只修该阶段范围内的问题
5. **回归**：重跑该阶段所有测试，确认全过
6. **打勾**：更新 checklist

## 脚本用法速查

```bash
# 单个测试
./scripts/run.sh --run-test HTTPRouteSimpleSameNamespace

# HTTP Core
./scripts/run.sh --core --conformance-profiles GATEWAY-HTTP

# HTTP Extended
./scripts/run.sh --extended --conformance-profiles GATEWAY-HTTP

# GRPC
./scripts/run.sh --conformance-profiles GATEWAY-GRPC --supported-features Gateway,GRPCRoute,ReferenceGrant

# TLS
./scripts/run.sh --conformance-profiles GATEWAY-TLS --supported-features Gateway,TLSRoute,ReferenceGrant

# 用 go test 正则跑一批
cd examples/gateway-api-conformance
go test ./... -run "TestConformance/HTTPRouteRedirect" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch
```
