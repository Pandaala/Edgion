# 阶段 4: TLS Passthrough Conformance

**测试数**: 8 (Core)
**预计耗时**: 1-2 天
**Profile**: GATEWAY-TLS
**Features**: Gateway, TLSRoute, ReferenceGrant

> ⚠️ GATEWAY-TLS profile 目前未声明。本阶段目标是跑通 Core tests 后再声明。

## 运行命令

```bash
# TLS Core
./scripts/run.sh --conformance-profiles GATEWAY-TLS \
  --supported-features "Gateway,TLSRoute,ReferenceGrant"

# 单个测试
./scripts/run.sh --run-test TLSRouteSimpleSameNamespace
```

## 前置工作

1. 在 `conformance_test.go` 中临时添加 `SupportTLSRoute` feature
2. 添加 `GatewayTLSConformanceProfileName` 到 `ConformanceProfiles`
3. 确认 TLSRoute CRD 已安装到集群

## TLSRoute Core — Passthrough (8 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | TLSRouteSimpleSameNamespace | [ ] | 最基本的 TLS passthrough |
| 2 | TLSRouteHostnameIntersection | [ ] | |
| 3 | TLSRouteInvalidBackendRefNonexistent | [ ] | |
| 4 | TLSRouteInvalidBackendRefUnknownKind | [ ] | |
| 5 | TLSRouteInvalidNoMatchingListenerHostname | [ ] | |
| 6 | TLSRouteInvalidNoMatchingListener | [ ] | |
| 7 | TLSRouteInvalidReferenceGrant | [ ] | |
| 8 | TLSRouteListenerPassthroughSupportedKinds | [ ] | |

## 预计需要修复

- `supportedKinds` 未在 listener status 中包含 TLSRoute
- `ResolvedRefs` condition 可能缺失或 reason 不匹配
- TLSRoute 的 Accepted condition reason 可能需要调整

## 通过后

在 `conformance_test.go` 中正式声明：
- `opts.SupportedFeatures.Insert(features.SupportTLSRoute)`
- `opts.ConformanceProfiles` 添加 `suite.GatewayTLSConformanceProfileName`
