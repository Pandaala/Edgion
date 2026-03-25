# 阶段 1: HTTP Core Conformance

**测试数**: 23
**预计耗时**: 1-2 天
**Profile**: GATEWAY-HTTP
**Features**: Gateway, HTTPRoute, ReferenceGrant

## 运行命令

```bash
# 全部 HTTP Core
./scripts/run.sh --core --conformance-profiles GATEWAY-HTTP

# 单个测试
./scripts/run.sh --run-test <TestName>
```

## Gateway Core (10 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | GatewayClassObservedGenerationBump | [ ] | |
| 2 | GatewayObservedGenerationBump | [ ] | |
| 3 | GatewayInvalidRouteKind | [ ] | |
| 4 | GatewayInvalidTLSConfiguration | [ ] | |
| 5 | GatewayModifyListeners | [ ] | |
| 6 | GatewayWithAttachedRoutes | [ ] | |
| 7 | GatewaySecretReferenceGrantAllInNamespace | [ ] | |
| 8 | GatewaySecretReferenceGrantSpecific | [ ] | |
| 9 | GatewaySecretInvalidReferenceGrant | [ ] | |
| 10 | GatewaySecretMissingReferenceGrant | [ ] | |

## HTTPRoute Core (13 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteSimpleSameNamespace | [ ] | 最基本的，应该首先通过 |
| 2 | HTTPRouteCrossNamespace | [ ] | |
| 3 | HTTPRouteExactPathMatching | [ ] | |
| 4 | HTTPRouteHeaderMatching | [ ] | |
| 5 | HTTPRouteHostnameIntersection | [ ] | |
| 6 | HTTPRouteListenerHostnameMatching | [ ] | |
| 7 | HTTPRouteMatching | [ ] | |
| 8 | HTTPRouteMatchingAcrossRoutes | [ ] | |
| 9 | HTTPRouteObservedGenerationBump | [ ] | |
| 10 | HTTPRoutePathMatchOrder | [ ] | |
| 11 | HTTPRouteRedirectHostAndStatus | [ ] | |
| 12 | HTTPRouteHTTPSListener | [ ] | |
| 13 | HTTPRouteServiceTypes | [ ] | |

## HTTPRoute Core — 权重和错误处理 (10 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteWeight | [ ] | |
| 2 | HTTPRouteDisallowedKind | [ ] | |
| 3 | HTTPRouteInvalidBackendRefUnknownKind | [ ] | |
| 4 | HTTPRouteInvalidCrossNamespaceBackendRef | [ ] | |
| 5 | HTTPRouteInvalidCrossNamespaceParentRef | [ ] | |
| 6 | HTTPRouteInvalidNonExistentBackendRef | [ ] | |
| 7 | HTTPRouteInvalidParentRefNotMatchingSectionName | [ ] | |
| 8 | HTTPRouteInvalidReferenceGrant | [ ] | |
| 9 | HTTPRoutePartiallyInvalidViaInvalidReferenceGrant | [ ] | |
| 10 | HTTPRouteReferenceGrant | [ ] | |

## 常见问题预判

- **status condition 不符合预期**：observedGeneration 未更新、condition reason 不匹配
- **ReferenceGrant 逻辑**：跨命名空间引用的允许/拒绝
- **hostname intersection**：listener hostname 与 route hostname 交集计算
