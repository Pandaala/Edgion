# 阶段 3: GRPC Conformance

**测试数**: 5
**预计耗时**: 0.5 天
**Profile**: GATEWAY-GRPC
**Features**: Gateway, GRPCRoute, ReferenceGrant

## 运行命令

```bash
# 全部 GRPC
./scripts/run.sh --conformance-profiles GATEWAY-GRPC \
  --supported-features "Gateway,GRPCRoute,ReferenceGrant"

# 含 Extended
./scripts/run.sh --conformance-profiles GATEWAY-GRPC \
  --supported-features "Gateway,GRPCRoute,ReferenceGrant,GRPCRouteNamedRouteRule"
```

## GRPCRoute Core (4 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | GRPCExactMethodMatching | [ ] | |
| 2 | GRPCRouteHeaderMatching | [ ] | |
| 3 | GRPCRouteListenerHostnameMatching | [ ] | |
| 4 | GRPCRouteWeight | [ ] | |

## GRPCRoute Extended (1 test)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | GRPCRouteNamedRule | [ ] | |

## 已知风险

- GRPC profile 已声明但从未实际跑过 conformance test
- 可能需要修 status condition（Accepted、ResolvedRefs 的 reason 字符串）
