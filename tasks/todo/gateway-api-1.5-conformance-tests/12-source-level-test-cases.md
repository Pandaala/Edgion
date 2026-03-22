# Conformance Tests - 源码级精确测试用例

从 Gateway API v1.5.0 上游源码提取的精确测试用例。每个条目包含精确的请求参数和期望响应。

---

## HTTPRouteRedirectPath (6 个用例)

**Route**: `redirect-path`, **Gateway**: `same-namespace`

| # | Request Path | Expected Status | Expected Redirect Path | Expected Redirect Host | 说明 |
|---|-------------|----------------|----------------------|----------------------|------|
| 1 | `/original-prefix/lemon` | 302 | `/replacement-prefix/lemon` | - | ReplacePrefixMatch |
| 2 | `/full/path/original` | 302 | `/full-path-replacement` | - | ReplaceFullPath |
| 3 | `/path-and-host` | 302 | `/replacement-prefix` | `example.org` | 路径+Host重定向 |
| 4 | `/path-and-status` | **301** | `/replacement-prefix` | - | 自定义状态码 |
| 5 | `/full-path-and-host` | 302 | `/replacement-full` | `example.org` | 全路径+Host |
| 6 | `/full-path-and-status` | **301** | `/replacement-full` | - | 全路径+自定义状态码 |

**关键实现点**:
- ReplacePrefixMatch: 保留前缀后的剩余路径
- ReplaceFullPath: 完全替换整个路径
- 可同时配置 hostname 和 statusCode

---

## HTTPRoute303Redirect (1 个用例)

**Route**: `redirect-303`, **Gateway**: `same-namespace`

| # | Method | Path | Expected Status |
|---|--------|------|----------------|
| 1 | POST | `/see-other` | **303** |

**关键**: 303 See Other 通常在 POST 后重定向到 GET

---

## HTTPRoute307Redirect (1 个用例)

| # | Method | Path | Expected Status |
|---|--------|------|----------------|
| 1 | GET | `/temporary` | **307** |

---

## HTTPRoute308Redirect (1 个用例)

| # | Method | Path | Expected Status |
|---|--------|------|----------------|
| 1 | GET | `/permanent` | **308** |

---

## HTTPRouteTimeoutRequest (3 个用例)

**Route**: `timeout-request`, **Gateway**: `same-namespace`

| # | Path | Delay | Expected Status | 说明 |
|---|------|-------|----------------|------|
| 1 | `/request-timeout` | 0s | 200 | 正常请求 |
| 2 | `/request-timeout?delay=1s` | 1s | **504** | 超过 request timeout |
| 3 | `/disable-request-timeout?delay=1s` | 1s | 200 | timeout=0 表示禁用 |

**关键**: `rules[].timeouts.request` 设为 0 表示禁用超时

---

## HTTPRouteRewritePath (6 个用例)

**Route**: `rewrite-path`, **Gateway**: `same-namespace`

| # | Request Path | Request Headers | Expected Backend Path | Expected Backend Headers | 说明 |
|---|-------------|----------------|---------------------|------------------------|------|
| 1 | `/prefix/one/two` | - | `/one/two` | - | ReplacePrefixMatch: 去掉 `/prefix` |
| 2 | `/strip-prefix/three` | - | `/three` | - | strip prefix |
| 3 | `/strip-prefix` | - | `/` | - | strip prefix 到根 |
| 4 | `/full/one/two` | - | `/one` | - | ReplaceFullPath |
| 5 | `/full-and-headers/one/two` | `X-Header-Set: original, X-Header-Add: original, X-Header-Remove: remove-val` | `/one` | `X-Header-Set: set-overwrites-values, X-Header-Add: original,header-add-1` AbsentHeaders: `X-Header-Remove` | 全路径重写+Header修改 |
| 6 | `/prefix-and-headers/one/two` | `X-Header-Set: original, X-Header-Add: original, X-Header-Remove: remove-val` | `/one/two` | `X-Header-Set: set-overwrites-values, X-Header-Add: original,header-add-1` AbsentHeaders: `X-Header-Remove` | 前缀重写+Header修改 |

---

## HTTPRouteCORS - YAML Manifest 详解

### Route 1: `cors-multiple-origins-methods-headers` (路径 `/cors-1`)
```yaml
cors:
  allowOrigins: ["https://www.foo.com", "https://*.bar.com"]
  allowMethods: [GET, OPTIONS]
  allowHeaders: ["x-header-1", "x-header-2"]
  exposeHeaders: ["x-header-3", "x-header-4"]
  allowCredentials: true
  maxAge: 3600
```

### Route 2: `cors-wildcard-methods` (路径 `/cors-2`)
```yaml
cors:
  allowOrigins: ["https://www.foo.com"]
  allowMethods: ["*"]
```

### Route 3: `cors-wildcard-origin` (路径 `/cors-wildcard-origin`)
```yaml
cors:
  allowOrigins: ["*"]
  allowMethods: ["PUT"]
  allowCredentials: false
```

### Route 4: `cors-wildcard-methods-headers` (路径 `/cors-wildcard-methods-headers` + `/cors-wildcard-methods-headers-unauth`)
```yaml
# 认证路径:
cors:
  allowOrigins: ["*"]
  allowMethods: ["*"]
  allowHeaders: ["*"]
  allowCredentials: true

# 非认证路径:
cors:
  allowOrigins: ["*"]
  allowMethods: ["*"]
  allowHeaders: ["*"]
  allowCredentials: false
```

### HTTPRouteCORS 测试用例 (20 个)

| # | Route Path | Method | Origin | Request Headers | Expected Status | Expected Response Headers | Absent Headers | 说明 |
|---|-----------|--------|--------|----------------|----------------|--------------------------|---------------|------|
| 1 | `/cors-1` | OPTIONS | `https://www.foo.com` | `Access-Control-Request-Method: GET` | 200 | `ACAO: https://www.foo.com`, `ACAM: GET,OPTIONS`, `ACAH: x-header-1,x-header-2`, `ACMA: 3600`, `ACAC: true` | - | 精确Origin匹配 preflight |
| 2 | `/cors-1` | OPTIONS | `https://sub.bar.com` | `Access-Control-Request-Method: GET` | 200 | `ACAO: https://sub.bar.com`, `ACAC: true` | - | 通配符 `*.bar.com` 匹配 |
| 3 | `/cors-1` | OPTIONS | `https://www.baz.com` | `Access-Control-Request-Method: GET` | 200 | - | `ACAO, ACAM, ACAH` | 不匹配的 Origin |
| 4 | `/cors-2` | OPTIONS | `https://www.foo.com` | `Access-Control-Request-Method: DELETE` | 200 | `ACAM` 包含 DELETE | - | wildcard method `*` |
| 5 | `/cors-wildcard-origin` | OPTIONS | `https://any.com` | `Access-Control-Request-Method: PUT` | 200 | `ACAO: *` | `ACAC` | wildcard origin, no credentials |
| 6 | `/cors-wildcard-methods-headers` | OPTIONS | `https://any.com` | `Access-Control-Request-Method: POST` | 200 | `ACAO: https://any.com`, `ACAC: true` | - | wildcard + credentials → echo origin |
| 7 | `/cors-wildcard-methods-headers-unauth` | OPTIONS | `https://any.com` | `Access-Control-Request-Method: POST` | 200 | `ACAO: *` | `ACAC` | wildcard + no credentials → `*` |

> ACAO = Access-Control-Allow-Origin, ACAM = Access-Control-Allow-Methods, ACAH = Access-Control-Allow-Headers, ACMA = Access-Control-Max-Age, ACAC = Access-Control-Allow-Credentials

### HTTPRouteCORSAllowCredentialsBehavior 测试用例 (3 个)

**Route**: `cors-allow-credentials`, **Origin**: `https://app.example`

| # | Method | Path | Request Headers | Expected Status | Expected Response Headers | Absent Headers |
|---|--------|------|----------------|----------------|--------------------------|---------------|
| 1 | GET | `/cors-behavior-creds-false` | `Origin: https://app.example, Cookie: sid=abc123, Authorization: Bearer test` | 200 | - | `Access-Control-Allow-Credentials` |
| 2 | GET | `/cors-behavior-creds-true` | `Origin: http://not-app.example, Cookie: sid=abc123, Authorization: Bearer test` | 200 | - | `Access-Control-Allow-Credentials` |
| 3 | GET | `/cors-behavior-creds-true` | `Origin: https://app.example, Cookie: sid=abc123, Authorization: Bearer test` | 200 | `ACAC: true, ACAO: https://app.example` | - |

**关键点**:
- `allowCredentials: false` → 不返回 `Access-Control-Allow-Credentials`
- `allowCredentials: true` 但 Origin 不匹配 → 不返回 credentials 头
- `allowCredentials: true` + Origin 匹配 → 返回 `Access-Control-Allow-Credentials: true` 且 `ACAO` 为具体 origin (不能是 `*`)

---

## TLSRouteSimpleSameNamespace

**Gateway**: `gateway-tlsroute` (mode=Passthrough), **Route**: `gateway-conformance-infra-test`

| # | 场景 | SNI | Expected | Backend TLS |
|---|------|-----|----------|-------------|
| 1 | TLS 请求匹配 TLSRoute | hostname from route | 到达 `tcp-backend` | **true** (passthrough，后端收到 TLS) |

**验证细节**:
- 使用 `tls-passthrough-checks-certificate` Secret 中的证书验证服务端
- `BackendIsTLS: true` — passthrough 模式下后端维持 TLS 连接
- 使用 `tcp.MakeTCPRequestAndExpectEventuallyValidResponse` 发送 TCP/TLS 请求

---

## TLSRouteTerminateSimpleSameNamespace

**Gateway**: `gateway-tlsroute-terminate` (mode=Terminate), **Route**: `tlsroute-terminated-test`

| # | 场景 | SNI | Expected Backend | Backend TLS | Hostname in Response |
|---|------|-----|-----------------|-------------|---------------------|
| 1 | TLS Terminate | hostname from route | `tcp-backend` | **false** (终止后明文) | 空 (无 SNI) |

**验证细节**:
- Gateway 终止 TLS 连接
- `BackendIsTLS: false` — 后端收到明文 TCP
- `Hostname: ""` — terminate 模式下后端不会看到 SNI 属性
- 使用 `tls-checks-ca-certificate` ConfigMap 中的 CA 证书验证 Gateway 的 TLS

---

## TLSRouteMixedTerminationSameNamespace

**Gateway**: 2 个 TLS listener (同端口，不同模式)

| # | 场景 | Route | SNI | Backend TLS | Hostname in Response |
|---|------|-------|-----|-------------|---------------------|
| 1 | Terminate 路径 | terminated route | terminate hostname | **false** | 空 |
| 2 | Passthrough 路径 | passthrough route | passthrough hostname | **true** | passthrough hostname |

**关键**: 同端口两个 listener，一个 Passthrough 一个 Terminate，根据 SNI 路由到不同模式

---

## ListenerSetHTTPRouting (36 个用例!)

这是最复杂的测试之一。测试 ListenerSet 添加的 listener 与 Gateway 原生 listener 的路由隔离。

**架构**:
- Gateway `gateway-with-listener-sets-http-routing` 有 2 个 listener:
  - `gateway-listener-1` → hostname `gateway-listener-1.com`
  - `gateway-listener-2` → hostname `gateway-listener-2.com`
- ListenerSet `listener-set-http-routing-1` 有 2 个 listener:
  - `listener-set-http-routing-1-listener-1` → hostname `listener-set-http-routing-1-listener-1.com`
  - `listener-set-http-routing-1-listener-2` → hostname `listener-set-http-routing-1-listener-2.com`
- ListenerSet `listener-set-http-routing-2` 有 2 个 listener:
  - `listener-set-http-routing-2-listener-1` → hostname `listener-set-http-routing-2-listener-1.com`
  - `listener-set-http-routing-2-listener-2` → hostname `listener-set-http-routing-2-listener-2.com`

**Routes**:
- `attaches-to-all-listeners` → parentRef: Gateway (所有 listener)
- `gateway-route` → parentRef: Gateway
- `gateway-section-route` → parentRef: Gateway, sectionName: `gateway-listener-1`
- `listener-set-http-routing-1-route` → parentRef: ListenerSet 1
- `listener-set-http-routing-1-section-route` → parentRef: ListenerSet 1, sectionName: `listener-set-http-routing-1-listener-1`
- `listener-set-http-routing-2-route` → parentRef: ListenerSet 2

### 测试矩阵 (Host × Path)

| Host | `/route` | `/gateway-route` | `/gateway-section-route` | `/ls-1-route` | `/ls-1-section-route` | `/ls-2-route` |
|------|----------|------------------|--------------------------|---------------|----------------------|---------------|
| gw-listener-1 | ✅ v1 | ✅ v2 | ✅ v3 | ❌ 404 | ❌ 404 | ❌ 404 |
| gw-listener-2 | ✅ v1 | ✅ v2 | ❌ 404 | ❌ 404 | ❌ 404 | ❌ 404 |
| ls-1-listener-1 | ✅ v1 | ❌ 404 | ❌ 404 | ✅ v2 | ✅ v3 | ❌ 404 |
| ls-1-listener-2 | ✅ v1 | ❌ 404 | ❌ 404 | ✅ v2 | ❌ 404 | ❌ 404 |
| ls-2-listener-1 | ✅ v1 | ❌ 404 | ❌ 404 | ❌ 404 | ❌ 404 | ✅ v2 |
| ls-2-listener-2 | ✅ v1 | ❌ 404 | ❌ 404 | ❌ 404 | ❌ 404 | ✅ v2 |

> ✅ v1/v2/v3 = 路由到 infra-backend-v1/v2/v3, ❌ 404 = 返回 404

**附加验证 - AttachedRoutes**:
- ListenerSet 1, listener-1: attachedRoutes = 3
- ListenerSet 1, listener-2: attachedRoutes = 2
- ListenerSet 2, listener-1: attachedRoutes = 2
- ListenerSet 2, listener-2: attachedRoutes = 2
- Gateway listener 需要有 2 个 attached listeners (来自 2 个 ListenerSet)

**关键实现要求**:
1. Route 的 parentRef 指向 ListenerSet 时，只在该 ListenerSet 的 listener 上生效
2. Route 的 parentRef 指向 Gateway 时，在 Gateway 自身的 listener 上生效，不在 ListenerSet 的 listener 上生效
3. Route 的 parentRef 指向 Gateway 且无 sectionName 时，在 Gateway 所有 listener + 所有 ListenerSet listener 上生效
4. sectionName 精确匹配单个 listener

---

## HTTPRouteRequestMirror (2 个用例)

**Route**: `request-mirror`, **Gateway**: `same-namespace`

| # | Path | Primary Backend | Mirror Backend | 额外验证 |
|---|------|----------------|---------------|---------|
| 1 | `/mirror` | `infra-backend-v1` | `infra-backend-v2` | 验证 mirror 请求到达 |
| 2 | `/mirror-and-modify-headers` | `infra-backend-v1` | `infra-backend-v2` | Header 修改 + mirror |

用例 2 详细 Header 操作:
- Request 带: `X-Header-Set: original, X-Header-Add: original, X-Header-Remove: remove-val`
- 期望后端收到: `X-Header-Set: set-overwrites-values, X-Header-Add: original,header-add-1`
- 期望后端不收到: `X-Header-Remove`
