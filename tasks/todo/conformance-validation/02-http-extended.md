# 阶段 2: HTTP Extended Conformance

**测试数**: 29
**预计耗时**: 2-3 天
**Profile**: GATEWAY-HTTP
**Features**: Core + 所有 HTTPRoute Extended features

## 运行命令

```bash
# 全部 HTTP Extended
./scripts/run.sh --extended --conformance-profiles GATEWAY-HTTP

# 按子功能跑（用 go test 正则）
cd examples/gateway-api-conformance
go test ./... -run "TestConformance/HTTPRouteRedirect" -v -timeout 10m -count=1 \
  -args -gateway-class=edgion -allow-crds-mismatch
```

## 匹配 (2 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteMethodMatching | [ ] | |
| 2 | HTTPRouteQueryParamMatching | [ ] | |

## 重定向 (7 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteRedirectPath | [ ] | |
| 2 | HTTPRouteRedirectPort | [ ] | |
| 3 | HTTPRouteRedirectPortAndScheme | [ ] | |
| 4 | HTTPRouteRedirectScheme | [ ] | |
| 5 | HTTPRoute303Redirect | [ ] | |
| 6 | HTTPRoute307Redirect | [ ] | |
| 7 | HTTPRoute308Redirect | [ ] | |

## 重写 (2 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteRewriteHost | [ ] | |
| 2 | HTTPRouteRewritePath | [ ] | |

## Header 修改 (3 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteBackendRequestHeaderModifier | [ ] | |
| 2 | HTTPRouteRequestHeaderModifierBackendWeights | [ ] | |
| 3 | HTTPRouteResponseHeaderModifier | [ ] | |

## 镜像 (3 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteRequestMirror | [ ] | |
| 2 | HTTPRouteRequestMultipleMirrors | [ ] | ⚠️ 需要 P1 缺失功能 |
| 3 | HTTPRouteRequestPercentageMirror | [ ] | |

## 超时 (2 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteTimeoutRequest | [ ] | |
| 2 | HTTPRouteTimeoutBackendRequest | [ ] | |

## 协议 (2 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteBackendProtocolH2C | [ ] | |
| 2 | HTTPRouteBackendProtocolWebSocket | [ ] | |

## ParentRef 端口 (3 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteListenerPortMatching | [ ] | |
| 2 | HTTPRouteInvalidParentRefSectionNameNotMatchingPort | [ ] | |
| 3 | HTTPRouteInvalidParentRefNotMatchingListenerPort | [ ] | |

## 其他 (5 tests)

| # | 测试名 | 状态 | 备注 |
|---|--------|------|------|
| 1 | HTTPRouteNamedRule | [ ] | |
| 2 | HTTPRouteCORS | [ ] | v1.5 新增 |
| 3 | HTTPRouteCORSAllowCredentialsBehavior | [ ] | v1.5 新增 |
| 4 | HTTPRouteHTTPSListenerDetectMisdirectedRequests | [ ] | |
| 5 | HTTPRouteListenerPortMatching | [ ] | |
