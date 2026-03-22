# HTTPRoute Conformance Tests (52 tests)

## Core Tests (23 tests)

### 1. HTTPRouteSimpleSameNamespace
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-simple-same-namespace.yaml`
- **描述**: 同命名空间内单个 HTTPRoute 挂载到 Gateway
- **测试逻辑**:
  1. 在 `gateway-conformance-infra` 命名空间创建 HTTPRoute `gateway-conformance-infra-test`
  2. 引用同命名空间的 Gateway `same-namespace`
  3. 验证 Gateway 和 HTTPRoute 被 accepted
  4. 验证 HTTPRoute 的 ResolvedRefs 条件为 True
  5. 发送 `GET /` 请求，期望:
     - 状态码 200
     - 后端: `infra-backend-v1`
     - 命名空间: `gateway-conformance-infra`

### 2. HTTPRouteCrossNamespace
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-cross-namespace.yaml`
- **描述**: 跨命名空间的 HTTPRoute 路由
- **测试逻辑**:
  1. 在 `gateway-conformance-app-backend` 命名空间创建 HTTPRoute
  2. 引用 `gateway-conformance-infra` 命名空间的 Gateway `all-namespaces`
  3. 验证跨命名空间路由被接受
  4. 发送请求验证流量到达正确的跨命名空间后端

### 3. HTTPRouteExactPathMatching
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-exact-path-matching.yaml`
- **描述**: 精确路径匹配
- **测试逻辑**:
  1. 创建带有 Exact 类型路径匹配的 HTTPRoute
  2. 测试用例:
     - `GET /one` → 后端 `infra-backend-v1` (精确匹配)
     - `GET /two` → 后端 `infra-backend-v2` (精确匹配)
     - `GET /one/` → 不匹配 (尾部斜杠不同)
     - `GET /one/two` → 不匹配 (路径更长)
     - `GET /` → 不匹配或默认后端

### 4. HTTPRouteHeaderMatching
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-header-matching.yaml`
- **描述**: HTTP 头部匹配路由
- **测试逻辑**:
  1. 创建带有 header 匹配条件的 HTTPRoute
  2. 测试用例:
     - 请求带 `Version: one` header → 路由到 backend-v1
     - 请求带 `Version: two` header → 路由到 backend-v2
     - 请求带多个 header 匹配 → 路由到对应后端
     - 请求不带匹配 header → 不匹配该规则
  3. Header 匹配是大小写不敏感的 (header name)
  4. Header value 匹配是精确的

### 5. HTTPRouteHostnameIntersection
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-hostname-intersection.yaml`
- **描述**: Gateway listener hostname 与 HTTPRoute hostname 的交集匹配
- **测试逻辑**:
  1. Gateway 定义多个 listener，各有不同 hostname
  2. HTTPRoute 定义多个 hostname
  3. 只有 Gateway listener hostname 和 HTTPRoute hostname 的交集才生效
  4. 测试各种 hostname 组合:
     - 精确匹配
     - 通配符 `*.example.com` 与具体 `foo.example.com`
     - 不匹配的 hostname 组合

### 6. HTTPRouteListenerHostnameMatching
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-listener-hostname-matching.yaml`
- **描述**: 监听器主机名匹配
- **测试逻辑**:
  1. Gateway 有多个 listener，每个有不同 hostname
  2. HTTPRoute 匹配特定 listener hostname
  3. 验证请求根据 Host header 路由到正确后端
  4. 测试通配符和精确主机名的交互

### 7. HTTPRouteMatching
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-matching.yaml`
- **描述**: 路径和头部的组合匹配
- **测试逻辑**:
  1. 创建 HTTPRoute，规则包含路径前缀和 header 匹配
  2. 测试用例:
     - `GET /` → `infra-backend-v1`
     - `GET /example` → `infra-backend-v1`
     - `GET /` + `Version: one` → `infra-backend-v1`
     - `GET /v2` → `infra-backend-v2`
     - `GET /v2/example` → `infra-backend-v2`
     - `GET /` + `Version: two` → `infra-backend-v2`
     - `GET /v2/` → `infra-backend-v2`
     - `GET /v2example` → `infra-backend-v1` (不是前缀匹配，/v2 != /v2example)
     - `GET /foo/v2/example` → `infra-backend-v1` (前缀不匹配)

### 8. HTTPRouteMatchingAcrossRoutes
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-matching-across-routes.yaml`
- **描述**: 跨多个 HTTPRoute 的匹配
- **测试逻辑**:
  1. 创建多个 HTTPRoute 绑定到同一 Gateway
  2. 验证请求根据匹配规则正确路由到不同 HTTPRoute 指定的后端
  3. 验证优先级和匹配顺序正确

### 9. HTTPRouteObservedGenerationBump
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-observed-generation-bump.yaml`
- **描述**: HTTPRoute 更新后 observedGeneration 递增
- **测试逻辑**:
  1. 创建 HTTPRoute
  2. 验证 status.conditions 中 observedGeneration 等于 metadata.generation
  3. 更新 HTTPRoute
  4. 验证 observedGeneration 跟随 generation 递增

### 10. HTTPRoutePathMatchOrder
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-path-match-order.yaml`
- **描述**: 路径匹配优先级顺序
- **测试逻辑**:
  1. 创建包含多个路径规则的 HTTPRoute
  2. 验证:
     - 更长的路径前缀优先于更短的
     - Exact 匹配优先于 PathPrefix
     - 同等长度前缀按规则顺序匹配
  3. 测试用例: `/path/one/match`, `/path/one`, `/path` 等

### 11. HTTPRouteRedirectHostAndStatus
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-redirect-host-and-status.yaml`
- **描述**: Host 重定向和状态码
- **测试逻辑**:
  1. 创建带 RequestRedirect filter 的 HTTPRoute
  2. 测试:
     - 重定向到新 hostname，默认 302 状态码
     - 重定向到新 hostname，自定义 301 状态码
     - 验证 Location header 包含正确的目标 host
     - 验证响应不包含 body (或为空)

### 12. HTTPRouteHTTPSListener
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-https-listener.yaml`
- **描述**: HTTPS 监听器路由
- **测试逻辑**:
  1. Gateway 有 HTTPS listener (443 端口，TLS terminate 模式)
  2. HTTPRoute 绑定到 HTTPS listener
  3. 发送 HTTPS 请求
  4. 验证请求到达正确后端
  5. 验证 TLS 终止后后端收到明文 HTTP

### 13. HTTPRouteServiceTypes
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-service-types.yaml`
- **描述**: 不同 Service 类型的路由
- **测试逻辑**:
  1. 创建引用不同类型 Service (ClusterIP, NodePort 等) 的 HTTPRoute
  2. 验证所有 Service 类型都能正确路由

### 14. HTTPRouteWeight
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-weight.yaml`
- **描述**: 加权路由 (流量分配)
- **测试逻辑**:
  1. 创建 HTTPRoute，backendRefs 有多个后端和不同 weight
  2. 发送大量请求
  3. 验证流量按 weight 比例分配
  4. 特殊情况: weight=0 的后端不应收到流量

### 15. HTTPRouteDisallowedKind
- **Features**: Gateway, HTTPRoute, TLSRoute
- **Manifest**: `httproute-disallowed-kind.yaml`
- **描述**: Gateway listener 不允许的路由类型
- **测试逻辑**:
  1. Gateway listener 的 allowedRoutes.kinds 仅允许 TLSRoute
  2. 创建 HTTPRoute 引用该 listener
  3. 验证 HTTPRoute 状态为 NotAccepted (条件: NotAllowedByListeners)

### 16. HTTPRouteInvalidBackendRefUnknownKind
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-invalid-backendref-unknown-kind.yaml`
- **描述**: 引用未知 Kind 的 backendRef
- **测试逻辑**:
  1. HTTPRoute 的 backendRef 引用不存在的 Kind
  2. 验证 HTTPRoute 的 ResolvedRefs 条件为 False
  3. 验证 reason 为 `InvalidKind`
  4. 请求该路由应返回 500

### 17. HTTPRouteInvalidCrossNamespaceBackendRef
- **Features**: Gateway, HTTPRoute, ReferenceGrant
- **Manifest**: `httproute-invalid-cross-namespace-backend-ref.yaml`
- **描述**: 无 ReferenceGrant 时跨命名空间 backendRef
- **测试逻辑**:
  1. HTTPRoute 引用其他命名空间的 Service 作为 backendRef
  2. 没有对应的 ReferenceGrant
  3. 验证 ResolvedRefs 条件为 False，reason: `RefNotPermitted`
  4. 请求应返回 500

### 18. HTTPRouteInvalidCrossNamespaceParentRef
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-invalid-cross-namespace-parent-ref.yaml`
- **描述**: 跨命名空间的 parentRef 未被允许
- **测试逻辑**:
  1. HTTPRoute 在一个命名空间，引用另一个命名空间的 Gateway
  2. Gateway 的 listener 只允许 Same namespace 的路由
  3. 验证 HTTPRoute 不被接受

### 19. HTTPRouteInvalidNonExistentBackendRef
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-invalid-nonexistent-backendref.yaml`
- **描述**: 引用不存在的 backend Service
- **测试逻辑**:
  1. HTTPRoute 的 backendRef 引用不存在的 Service
  2. 验证 ResolvedRefs 条件为 False，reason: `BackendNotFound`
  3. 请求应返回 500

### 20. HTTPRouteInvalidParentRefNotMatchingSectionName
- **Features**: Gateway, HTTPRoute
- **Manifest**: `httproute-invalid-parentref-not-matching-section-name.yaml`
- **描述**: parentRef 的 sectionName 不匹配任何 listener
- **测试逻辑**:
  1. HTTPRoute 的 parentRef 指定了不存在的 sectionName
  2. 验证 Accepted 条件为 False
  3. reason: `NoMatchingParent`

### 21. HTTPRouteInvalidReferenceGrant
- **Features**: Gateway, HTTPRoute, ReferenceGrant
- **Manifest**: `httproute-invalid-reference-grant.yaml`
- **描述**: ReferenceGrant 不匹配的情况
- **测试逻辑**:
  1. 存在 ReferenceGrant 但不允许目标资源
  2. HTTPRoute 引用需要 ReferenceGrant 的跨命名空间资源
  3. 验证请求被拒绝 (500)
  4. ResolvedRefs 为 False

### 22. HTTPRoutePartiallyInvalidViaInvalidReferenceGrant
- **Features**: Gateway, HTTPRoute, ReferenceGrant
- **Manifest**: `httproute-partially-invalid-via-invalid-reference-grant.yaml`
- **描述**: 部分 backendRef 因 ReferenceGrant 无效
- **测试逻辑**:
  1. HTTPRoute 有多个 backendRef，其中部分有效、部分因缺少 ReferenceGrant 无效
  2. 验证有效的 backendRef 仍然可路由
  3. 无效的 backendRef 被标记但不影响有效部分
  4. 状态条件反映部分无效状态

### 23. HTTPRouteReferenceGrant
- **Features**: Gateway, HTTPRoute, ReferenceGrant
- **Manifest**: `httproute-reference-grant.yaml`
- **描述**: ReferenceGrant 允许跨命名空间引用
- **测试逻辑**:
  1. 在目标命名空间创建 ReferenceGrant
  2. HTTPRoute 跨命名空间引用 backend Service
  3. 验证 ResolvedRefs 条件为 True
  4. 请求成功路由到跨命名空间后端

---

## Extended Tests - 匹配 (2 tests)

### 24. HTTPRouteMethodMatching
- **Features**: Gateway, HTTPRoute, HTTPRouteMethodMatching
- **Manifest**: `httproute-method-matching.yaml`
- **描述**: HTTP 方法匹配
- **测试逻辑**:
  1. 创建 HTTPRoute，规则基于 HTTP 方法匹配
  2. 测试用例:
     - `POST /` → 路由到 backend-v1
     - `GET /` → 路由到 backend-v2 (或默认)
     - 不同方法 (PUT, DELETE, PATCH) 匹配不同后端

### 25. HTTPRouteQueryParamMatching
- **Features**: Gateway, HTTPRoute, HTTPRouteQueryParamMatching
- **Manifest**: `httproute-query-param-matching.yaml`
- **描述**: URL 查询参数匹配
- **测试逻辑**:
  1. 创建 HTTPRoute，规则基于 query parameter 匹配
  2. 测试用例:
     - `GET /?animal=whale` → backend-v1
     - `GET /?animal=dolphin` → backend-v2
     - `GET /?animal=whale&color=blue` → 匹配 (额外参数不影响)
     - `GET /` (无参数) → 不匹配该规则
  3. Exact 类型参数匹配

---

## Extended Tests - 重定向 (8 tests)

### 26. HTTPRouteRedirectPath
- **Features**: Gateway, HTTPRoute, HTTPRoutePathRedirect
- **Manifest**: `httproute-redirect-path.yaml`
- **描述**: 路径重定向
- **测试逻辑**:
  1. 创建带 RequestRedirect filter 的 HTTPRoute，配置路径替换
  2. 测试:
     - `ReplaceFullPath`: `GET /original` → 重定向到 `/replacement`
     - `ReplacePrefixMatch`: `GET /prefix/rest` → 重定向到 `/new-prefix/rest`
  3. 验证 Location header 的路径部分正确

### 27. HTTPRouteRedirectPort
- **Features**: Gateway, HTTPRoute, HTTPRoutePortRedirect
- **Manifest**: `httproute-redirect-port.yaml`
- **描述**: 端口重定向
- **测试逻辑**:
  1. 创建带 RequestRedirect filter 的 HTTPRoute，配置端口
  2. 发送请求到默认端口
  3. 验证 Location header 包含指定的端口号
  4. 状态码 302

### 28. HTTPRouteRedirectPortAndScheme
- **Features**: Gateway, HTTPRoute, HTTPRoutePortRedirect, GatewayPort8080
- **Manifest**: `httproute-redirect-port-and-scheme.yaml`
- **描述**: 端口和协议组合重定向
- **测试逻辑**:
  1. Gateway 监听 8080 端口
  2. HTTPRoute 配置同时重定向端口和 scheme
  3. 验证 Location header 同时包含新 scheme 和新端口

### 29. HTTPRouteRedirectScheme
- **Features**: Gateway, HTTPRoute, HTTPRouteSchemeRedirect
- **Manifest**: `httproute-redirect-scheme.yaml`
- **描述**: 协议 scheme 重定向 (http→https)
- **测试逻辑**:
  1. 创建带 scheme 重定向的 HTTPRoute
  2. 发送 HTTP 请求
  3. 验证 Location header 的 scheme 从 http 变为 https
  4. 验证状态码 302

### 30. HTTPRoute303Redirect
- **Features**: Gateway, HTTPRoute, HTTPRoute303RedirectStatusCode
- **Manifest**: `httproute-303-redirect.yaml`
- **描述**: 303 See Other 重定向
- **测试逻辑**:
  1. 创建 RequestRedirect filter，statusCode=303
  2. 发送请求
  3. 验证响应状态码为 303
  4. 验证 Location header 正确

### 31. HTTPRoute307Redirect
- **Features**: Gateway, HTTPRoute, HTTPRoute307RedirectStatusCode
- **Manifest**: `httproute-307-redirect.yaml`
- **描述**: 307 Temporary Redirect
- **测试逻辑**:
  1. 创建 RequestRedirect filter，statusCode=307
  2. 发送请求
  3. 验证响应状态码为 307
  4. 验证 Location header 正确
  5. 307 保留原始请求方法

### 32. HTTPRoute308Redirect
- **Features**: Gateway, HTTPRoute, HTTPRoute308RedirectStatusCode
- **Manifest**: `httproute-308-redirect.yaml`
- **描述**: 308 Permanent Redirect
- **测试逻辑**:
  1. 创建 RequestRedirect filter，statusCode=308
  2. 发送请求
  3. 验证响应状态码为 308
  4. 验证 Location header 正确
  5. 308 保留原始请求方法 (永久版 307)

---

## Extended Tests - 重写 (2 tests)

### 33. HTTPRouteRewriteHost
- **Features**: Gateway, HTTPRoute, HTTPRouteHostRewrite
- **Manifest**: `httproute-rewrite-host.yaml`
- **描述**: 请求 Host header 重写
- **测试逻辑**:
  1. 创建带 URLRewrite filter 的 HTTPRoute，配置 hostname 重写
  2. 发送请求到原始 Host
  3. 验证后端收到的请求 Host header 为重写后的值
  4. 后端 echo 回请求的 Host header 以供验证

### 34. HTTPRouteRewritePath
- **Features**: Gateway, HTTPRoute, HTTPRoutePathRewrite
- **Manifest**: `httproute-rewrite-path.yaml`
- **描述**: 请求路径重写
- **测试逻辑**:
  1. 创建带 URLRewrite filter 的 HTTPRoute
  2. 测试:
     - `ReplaceFullPath`: `GET /original` → 后端收到 `GET /replacement`
     - `ReplacePrefixMatch`: `GET /prefix/rest` → 后端收到 `GET /new-prefix/rest`
  3. 验证后端收到的路径正确

---

## Extended Tests - Header 修改 (3 tests)

### 35. HTTPRouteBackendRequestHeaderModifier
- **Features**: Gateway, HTTPRoute, HTTPRouteBackendRequestHeaderModification
- **Manifest**: `httproute-backend-request-header-modifier.yaml`
- **描述**: 后端请求头修改 (backendRef 级别)
- **测试逻辑**:
  1. 创建 HTTPRoute，在 backendRef 上配置 RequestHeaderModifier filter
  2. 测试:
     - `set`: 设置新 header 或覆盖已有 header
     - `add`: 添加 header (可与已有值并存)
     - `remove`: 删除指定 header
  3. 验证后端收到的请求 header 正确修改
  4. 注意: 这是 backendRef 级别，不是 rule 级别

### 36. HTTPRouteRequestHeaderModifierBackendWeights
- **Features**: Gateway, HTTPRouteBackendRequestHeaderModification, HTTPRoute
- **Manifest**: `httproute-request-header-modifier-backend-weights.yaml`
- **描述**: 带权重的后端请求头修改
- **测试逻辑**:
  1. 多个 backendRef 有不同 weight 和不同 header modifier
  2. 验证每个后端收到各自对应的 header 修改
  3. 流量按 weight 分配

### 37. HTTPRouteResponseHeaderModifier
- **Features**: Gateway, HTTPRoute, HTTPRouteResponseHeaderModification
- **Manifest**: `httproute-response-header-modifier.yaml`
- **描述**: 响应头修改
- **测试逻辑**:
  1. 创建 HTTPRoute，配置 ResponseHeaderModifier filter
  2. 测试:
     - `set`: 设置响应 header
     - `add`: 添加响应 header
     - `remove`: 删除响应 header
  3. 验证客户端收到的响应 header 正确修改
  4. 后端返回的原始 header 被网关修改后转发

---

## Extended Tests - 请求镜像 (3 tests)

### 38. HTTPRouteRequestMirror
- **Features**: Gateway, HTTPRoute, HTTPRouteRequestMirror
- **Manifest**: `httproute-request-mirror.yaml`
- **描述**: 请求镜像到另一个后端
- **测试逻辑**:
  1. 创建 HTTPRoute，配置 RequestMirror filter
  2. 发送请求
  3. 验证:
     - 主后端收到请求并正常响应
     - 镜像后端也收到完全相同的请求
     - 客户端只收到主后端的响应
  4. 镜像是异步的，不影响主请求响应

### 39. HTTPRouteRequestMultipleMirrors
- **Features**: Gateway, HTTPRoute, HTTPRouteRequestMirror, HTTPRouteRequestMultipleMirrors
- **Manifest**: `httproute-request-multiple-mirrors.yaml`
- **描述**: 请求镜像到多个后端
- **测试逻辑**:
  1. 创建 HTTPRoute，配置多个 RequestMirror filter
  2. 发送请求
  3. 验证所有镜像目标都收到请求
  4. 客户端只收到主后端响应

### 40. HTTPRouteRequestPercentageMirror
- **Features**: Gateway, HTTPRoute, HTTPRouteRequestPercentageMirror
- **Manifest**: `httproute-request-percentage-mirror.yaml`
- **描述**: 按比例镜像请求
- **测试逻辑**:
  1. 创建 HTTPRoute，配置带 percentage 的 RequestMirror filter (如 fraction.numerator)
  2. 发送大量请求
  3. 验证镜像后端收到的请求数量大约符合配置的百分比

---

## Extended Tests - 超时 (2 tests)

### 41. HTTPRouteTimeoutRequest
- **Features**: Gateway, HTTPRoute, HTTPRouteRequestTimeout
- **Manifest**: `httproute-timeout-request.yaml`
- **描述**: 请求超时设置
- **测试逻辑**:
  1. 创建 HTTPRoute，配置 `rules[].timeouts.request`
  2. 后端延迟响应超过超时时间
  3. 验证网关返回 504 Gateway Timeout
  4. 后端快速响应时正常返回 200

### 42. HTTPRouteTimeoutBackendRequest
- **Features**: Gateway, HTTPRoute, HTTPRouteBackendTimeout
- **Manifest**: `httproute-timeout-backend-request.yaml`
- **描述**: 后端请求超时
- **测试逻辑**:
  1. 创建 HTTPRoute，配置 `rules[].timeouts.backendRequest`
  2. 后端延迟响应
  3. 验证超时时返回 504
  4. backendRequest timeout ≤ request timeout
  5. backendRequest 只计算到后端响应的时间，不包含客户端传输

---

## Extended Tests - 协议 (2 tests)

### 43. HTTPRouteBackendProtocolH2C
- **Features**: Gateway, HTTPRoute, HTTPRouteBackendProtocolH2C
- **Manifest**: `httproute-backend-protocol-h2c.yaml`
- **描述**: H2C (HTTP/2 cleartext) 后端协议
- **测试逻辑**:
  1. 创建 HTTPRoute，backendRef 的 Service 带有 `appProtocol: kubernetes.io/h2c`
  2. 验证网关使用 HTTP/2 明文连接后端
  3. 后端确认收到 HTTP/2 请求

### 44. HTTPRouteBackendProtocolWebSocket
- **Features**: Gateway, HTTPRoute, HTTPRouteBackendProtocolWebSocket
- **Manifest**: `httproute-backend-protocol-websocket.yaml`
- **描述**: WebSocket 后端协议
- **测试逻辑**:
  1. 创建 HTTPRoute，backendRef 的 Service 带有 `appProtocol: kubernetes.io/ws`
  2. 客户端发送 WebSocket 升级请求
  3. 验证 WebSocket 连接成功建立
  4. 验证双向通信正常

---

## Extended Tests - ParentRef 端口 (3 tests)

### 45. HTTPRouteListenerPortMatching
- **Features**: Gateway, HTTPRoute, HTTPRouteParentRefPort
- **Manifest**: `httproute-listener-port-matching.yaml`
- **描述**: parentRef 中指定端口来选择 listener
- **测试逻辑**:
  1. Gateway 有多个 listener 在不同端口
  2. HTTPRoute 的 parentRef 通过 port 字段选择特定 listener
  3. 验证路由只在指定端口生效

### 46. HTTPRouteInvalidParentRefSectionNameNotMatchingPort
- **Features**: Gateway, HTTPRoute, HTTPRouteParentRefPort
- **Manifest**: `httproute-invalid-parentref-section-name-not-matching-port.yaml`
- **描述**: parentRef 的 sectionName 和 port 不匹配
- **测试逻辑**:
  1. HTTPRoute parentRef 同时指定 sectionName 和 port，但两者指向不同 listener
  2. 验证 Accepted 条件为 False

### 47. HTTPRouteInvalidParentRefNotMatchingListenerPort
- **Features**: Gateway, HTTPRoute, HTTPRouteDestinationPortMatching
- **Manifest**: `httproute-invalid-parentref-not-matching-listener-port.yaml`
- **描述**: parentRef 端口不匹配任何 listener
- **测试逻辑**:
  1. HTTPRoute parentRef 指定的端口没有对应的 Gateway listener
  2. 验证 Accepted 条件为 False

---

## Extended Tests - 命名规则 (1 test)

### 48. HTTPRouteNamedRule
- **Features**: Gateway, HTTPRoute, HTTPRouteNamedRouteRule
- **Manifest**: `httproute-named-rule.yaml`
- **描述**: 命名路由规则
- **测试逻辑**:
  1. 创建 HTTPRoute，rules 带有 `name` 字段
  2. 验证路由正常工作
  3. 验证 status 中反映了命名规则

---

## Extended Tests - CORS (2 tests)

### 49. HTTPRouteCORS
- **Features**: Gateway, HTTPRoute, HTTPRouteCORS
- **Manifest**: `httproute-cors.yaml`
- **描述**: CORS (跨域资源共享) 支持
- **测试逻辑**:
  1. 创建 HTTPRoute，配置 CORS filter
  2. 发送 preflight 请求 (OPTIONS + Origin + Access-Control-Request-Method)
  3. 验证响应包含正确的 CORS 头:
     - `Access-Control-Allow-Origin`
     - `Access-Control-Allow-Methods`
     - `Access-Control-Allow-Headers`
     - `Access-Control-Max-Age`
  4. 发送普通请求带 Origin header
  5. 验证响应包含 `Access-Control-Allow-Origin`
  6. 验证不允许的 Origin 被拒绝

### 50. HTTPRouteCORSAllowCredentialsBehavior
- **Features**: Gateway, HTTPRoute, HTTPRouteCORS
- **Manifest**: `httproute-cors-allow-credentials-behavior.yaml`
- **描述**: CORS Allow-Credentials 行为
- **测试逻辑**:
  1. 创建 HTTPRoute，CORS filter 配置 `allowCredentials: true`
  2. 验证 preflight 响应包含 `Access-Control-Allow-Credentials: true`
  3. 验证当 allowCredentials=true 时，`Access-Control-Allow-Origin` 不能为 `*`
  4. 验证普通请求也返回 credentials 相关头

---

## Extended Tests - HTTPS 检测 (1 test)

### 51. HTTPRouteHTTPSListenerDetectMisdirectedRequests
- **Features**: Gateway, GatewayHTTPSListenerDetectMisdirectedRequests, HTTPRoute
- **Manifest**: `httproute-https-listener-detect-misdirected-requests.yaml`
- **描述**: HTTPS 监听器检测错误路由的请求 (421 Misdirected Request)
- **测试逻辑**:
  1. Gateway 有 HTTPS listener 绑定特定 hostname
  2. 客户端通过 TLS 连接到正确的 hostname，但发送 Host header 为不同的 hostname
  3. 验证返回 421 Misdirected Request
  4. SNI 和 Host header 不匹配时的行为

---

## Extended Tests - Listener 隔离 (1 test, 与 Gateway tests 重叠)

### 52. GatewayHTTPListenerIsolation
- **Features**: Gateway, GatewayHTTPListenerIsolation, HTTPRoute
- **Manifest**: `gateway-http-listener-isolation.yaml`
- **描述**: HTTP listener 之间的路由隔离
- **测试逻辑**:
  1. Gateway 有多个 HTTP listener，每个有不同的 hostname
  2. 不同 HTTPRoute 绑定到不同 listener (通过 sectionName)
  3. 验证发送到 listener A 的请求只能路由到绑定 listener A 的 HTTPRoute
  4. 验证 listener 之间不会交叉路由
