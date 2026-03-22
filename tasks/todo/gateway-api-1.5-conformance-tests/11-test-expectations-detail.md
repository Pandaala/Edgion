# Conformance Tests - 详细请求/响应期望

本文件记录每个测试的精确 HTTP 请求和期望响应，用于逐条核对功能实现。

---

## HTTPRouteSimpleSameNamespace

**Gateway**: `same-namespace` (ns: `gateway-conformance-infra`)
**Route**: `gateway-conformance-infra-test`

| # | Method | Path | Headers | Expected Status | Expected Backend | Expected NS |
|---|--------|------|---------|----------------|-----------------|-------------|
| 1 | GET | `/` | - | 200 | `infra-backend-v1` | `gateway-conformance-infra` |

**前置条件**:
- Gateway 和 HTTPRoute 必须 Accepted
- HTTPRoute ResolvedRefs 条件为 True

---

## HTTPRouteMatching

**Gateway**: `same-namespace`
**Route**: `matching`

| # | Method | Path | Headers | Expected Backend |
|---|--------|------|---------|-----------------|
| 1 | GET | `/` | - | `infra-backend-v1` |
| 2 | GET | `/example` | - | `infra-backend-v1` |
| 3 | GET | `/` | `Version: one` | `infra-backend-v1` |
| 4 | GET | `/v2` | - | `infra-backend-v2` |
| 5 | GET | `/v2/example` | - | `infra-backend-v2` |
| 6 | GET | `/` | `Version: two` | `infra-backend-v2` |
| 7 | GET | `/v2/` | - | `infra-backend-v2` |
| 8 | GET | `/v2example` | - | `infra-backend-v1` |
| 9 | GET | `/foo/v2/example` | - | `infra-backend-v1` |

**关键点**:
- `/v2` 是 PathPrefix 匹配，`/v2example` 不匹配因为 PathPrefix 要求完整路径段
- Header 匹配与路径匹配是 AND 关系

---

## HTTPRouteExactPathMatching

**Gateway**: `same-namespace`

| # | Path | Match Type | Expected |
|---|------|-----------|----------|
| 1 | `/one` | Exact | 匹配 → backend-v1 |
| 2 | `/two` | Exact | 匹配 → backend-v2 |
| 3 | `/one/` | - | 不匹配 (尾部斜杠) |
| 4 | `/ONE` | - | 不匹配 (大小写敏感) |

---

## HTTPRouteHeaderMatching

**Gateway**: `same-namespace`

| # | Path | Request Headers | Expected Backend |
|---|------|----------------|-----------------|
| 1 | `/` | `Version: one` | `infra-backend-v1` |
| 2 | `/` | `Version: two` | `infra-backend-v2` |
| 3 | `/` | `Version: one`, `Color: orange` | 匹配两个条件的规则 |
| 4 | `/` | (无匹配 header) | 默认后端或无匹配 |

**关键点**:
- Header name 匹配大小写不敏感
- Header value 匹配大小写敏感
- 多个 header 条件是 AND 关系

---

## HTTPRoutePathMatchOrder

**Gateway**: `same-namespace`

**优先级规则**:
1. Exact 匹配 > PathPrefix 匹配
2. 更长的 PathPrefix > 更短的 PathPrefix
3. 同长度 PathPrefix 按规则顺序

| # | Path | Expected Backend | 原因 |
|---|------|-----------------|------|
| 1 | `/path/one/match` | 最长前缀匹配的后端 | 更长的 `/path/one/match` 优先 |
| 2 | `/path/one` | 中间长度匹配 | `/path/one` 前缀 |
| 3 | `/path` | 最短前缀匹配 | `/path` 前缀 |
| 4 | `/path/one/match` (Exact) | Exact 匹配后端 | Exact 优先于 PathPrefix |

---

## HTTPRouteHostnameIntersection

**Gateway**: 多个 listener，各有 hostname

| # | Host Header | Listener | Route Hostname | 是否路由 |
|---|-------------|----------|---------------|---------|
| 1 | `foo.example.com` | `*.example.com` | `foo.example.com` | 是 (交集存在) |
| 2 | `bar.example.com` | `*.example.com` | `foo.example.com` | 否 (Host 不匹配 route) |
| 3 | `foo.other.com` | `*.example.com` | `foo.other.com` | 否 (Host 不匹配 listener) |

---

## HTTPRouteWeight

**Gateway**: `same-namespace`

| backendRef | weight | 期望流量比例 |
|-----------|--------|------------|
| `infra-backend-v1` | 50 | ~50% |
| `infra-backend-v2` | 50 | ~50% |

**特殊情况**:
- weight=0 的后端不应收到任何流量
- 所有 weight=0 时行为未定义

---

## HTTPRouteRedirectHostAndStatus

| # | Request Path | Redirect Hostname | Redirect StatusCode | Expected Location |
|---|-------------|------------------|--------------------|--------------------|
| 1 | `/` | `example.com` | 302 (默认) | `http://example.com/` |
| 2 | `/` | `example.org` | 301 | `http://example.org/` |

**验证点**:
- 响应状态码正确
- Location header 包含正确的 hostname
- 路径部分保留

---

## HTTPRouteRedirectPath

| # | Request Path | Redirect Type | Config | Expected Location Path |
|---|-------------|--------------|--------|----------------------|
| 1 | `/original` | ReplaceFullPath | `/replacement` | `/replacement` |
| 2 | `/prefix/rest` | ReplacePrefixMatch | prefix=`/prefix`, replacement=`/new-prefix` | `/new-prefix/rest` |
| 3 | `/prefix` | ReplacePrefixMatch | prefix=`/prefix`, replacement=`/new-prefix` | `/new-prefix` |
| 4 | `/prefix/` | ReplacePrefixMatch | prefix=`/prefix`, replacement=`/new-prefix` | `/new-prefix/` |

---

## HTTPRouteRedirectScheme

| # | Request Scheme | Config | Expected Location Scheme |
|---|---------------|--------|------------------------|
| 1 | `http` | scheme=`https` | `https` |

---

## HTTPRoute303/307/308Redirect

| Test | StatusCode | 保留方法 | 永久性 |
|------|-----------|---------|--------|
| HTTPRoute303Redirect | 303 | 否 (GET) | 否 |
| HTTPRoute307Redirect | 307 | 是 | 否 |
| HTTPRoute308Redirect | 308 | 是 | 是 |

---

## HTTPRouteRewriteHost

| # | Request Host | Rewrite Config | Backend 收到的 Host |
|---|-------------|---------------|-------------------|
| 1 | `original.example.com` | hostname=`rewritten.example.com` | `rewritten.example.com` |

---

## HTTPRouteRewritePath

| # | Request Path | Rewrite Type | Config | Backend 收到的 Path |
|---|-------------|-------------|--------|-------------------|
| 1 | `/original` | ReplaceFullPath | `/replacement` | `/replacement` |
| 2 | `/prefix/rest` | ReplacePrefixMatch | prefix=`/prefix`, replacement=`/new` | `/new/rest` |

---

## HTTPRouteResponseHeaderModifier

| # | Path | 操作 | Header Name | Header Value | 验证 |
|---|------|------|------------|-------------|------|
| 1 | `/set` | set | `X-Header-Set` | `set-overwrites-values` | 响应包含该 header |
| 2 | `/add` | add | `X-Header-Add` | `add-appends-values` | 响应包含该 header (可追加) |
| 3 | `/remove` | remove | `X-Header-Remove` | - | 响应不包含该 header |
| 4 | `/case-insensitivity` | set | `X-Header-Set` | `header-set` | header name 大小写不敏感 |

---

## HTTPRouteBackendRequestHeaderModifier

| # | Path | 操作 | Header Name | Value | Backend 验证 |
|---|------|------|------------|-------|------------|
| 1 | `/set` | set | `X-Header-Set-1` | `header-set-1` | 后端收到新 header |
| 2 | `/set` | set | `X-Header-Set-2` | `header-set-2` | 覆盖已有 header |
| 3 | `/add` | add | `X-Header-Add-1` | `header-add-1` | 追加到已有值 |
| 4 | `/remove` | remove | `X-Header-Remove-1` | - | 后端不收到该 header |

---

## HTTPRouteRequestMirror

| # | Request Path | Primary Backend | Mirror Backend | 客户端响应来源 |
|---|-------------|----------------|---------------|-------------|
| 1 | `/mirror` | `infra-backend-v1` | `infra-backend-v2` | 仅 primary |

**验证**: mirror backend 收到相同请求但不影响客户端响应

---

## HTTPRouteTimeoutRequest

| # | Path | Timeout Config | Backend Delay | Expected Status |
|---|------|---------------|---------------|----------------|
| 1 | `/timeout` | request=5s | 0s | 200 |
| 2 | `/timeout` | request=5s | 10s | 504 |

---

## HTTPRouteTimeoutBackendRequest

| # | Path | Timeout Config | Backend Delay | Expected Status |
|---|------|---------------|---------------|----------------|
| 1 | `/timeout` | backendRequest=5s | 0s | 200 |
| 2 | `/timeout` | backendRequest=5s | 10s | 504 |

---

## HTTPRouteMethodMatching

| # | Method | Path | Expected Backend |
|---|--------|------|-----------------|
| 1 | POST | `/` | `infra-backend-v1` |
| 2 | GET | `/` | `infra-backend-v2` |

---

## HTTPRouteQueryParamMatching

| # | Path + Query | Expected Backend |
|---|-------------|-----------------|
| 1 | `/?animal=whale` | `infra-backend-v1` |
| 2 | `/?animal=dolphin` | `infra-backend-v2` |
| 3 | `/?animal=whale&color=blue` | `infra-backend-v1` (额外参数不影响) |
| 4 | `/` (无 query) | 不匹配 |

---

## HTTPRouteCORS

### Preflight 请求
| # | Method | Headers | Expected Response Headers |
|---|--------|---------|--------------------------|
| 1 | OPTIONS | `Origin: http://allowed.example.com`, `Access-Control-Request-Method: GET` | `Access-Control-Allow-Origin: http://allowed.example.com`, `Access-Control-Allow-Methods` 包含 GET |
| 2 | OPTIONS | `Origin: http://disallowed.example.com` | 无 CORS 头或 403 |

### 普通请求
| # | Method | Headers | Expected Response Headers |
|---|--------|---------|--------------------------|
| 1 | GET | `Origin: http://allowed.example.com` | `Access-Control-Allow-Origin: http://allowed.example.com` |

---

## HTTPRouteCORSAllowCredentialsBehavior

| # | 场景 | Expected |
|---|------|----------|
| 1 | allowCredentials=true, specific origin | `Access-Control-Allow-Credentials: true`, `Access-Control-Allow-Origin: <specific-origin>` (不能为 `*`) |

---

## HTTPRouteBackendProtocolH2C

| # | Request | Backend appProtocol | 验证 |
|---|---------|--------------------|----- |
| 1 | HTTP/1.1 到 Gateway | `kubernetes.io/h2c` | Gateway 用 HTTP/2 cleartext 连接后端 |

---

## HTTPRouteBackendProtocolWebSocket

| # | Request | Backend appProtocol | 验证 |
|---|---------|--------------------|----- |
| 1 | WebSocket Upgrade | `kubernetes.io/ws` | WebSocket 连接成功，双向通信正常 |

---

## TLSRouteSimpleSameNamespace

**Gateway**: TLS listener, mode=Passthrough
**Route**: TLSRoute with hostname matching

| # | SNI | Expected | 验证 |
|---|-----|----------|------|
| 1 | 匹配 hostname | 透传到后端 | 后端收到完整 TLS 连接 |
| 2 | 不匹配 hostname | 不路由 | 连接被拒绝 |

---

## GatewayWithAttachedRoutes

| 步骤 | 操作 | Expected attachedRoutes |
|------|------|----------------------|
| 1 | 创建 Gateway | 0 |
| 2 | 创建 HTTPRoute 绑定 | 1 |
| 3 | 创建第二个 HTTPRoute | 2 |
| 4 | 删除一个 HTTPRoute | 1 |

---

## GatewayModifyListeners

| 步骤 | 操作 | Expected |
|------|------|----------|
| 1 | 创建 Gateway 1个 listener | 1 个 Programmed listener |
| 2 | 添加第2个 listener | 2 个 Programmed listener |
| 3 | 删除第1个 listener | 只剩第2个 |
| 4 | 修改 listener 端口 | 新端口生效 |

---

## GatewayInvalidRouteKind

| 场景 | allowedRoutes.kinds | Expected |
|------|--------------------|---------  |
| 无效 Kind | `[{group: "example.com", kind: "FooRoute"}]` | ResolvedRefs=False, reason=InvalidRouteKinds |
| 有效 Kind | `[{group: "gateway.networking.k8s.io", kind: "HTTPRoute"}]` | supportedKinds 包含 HTTPRoute |
