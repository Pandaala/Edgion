# Gateway API v1.5.0 Conformance Tests Checklist

用于逐一核对 Edgion 实现的功能是否满足 conformance 测试要求。

## 标记说明
- [ ] 未验证
- [x] 已通过
- [~] 部分通过/有已知问题
- [N/A] 不适用于 Edgion (如 Mesh)

---

## GATEWAY-HTTP Profile

### Gateway Core
- [ ] GatewayClassObservedGenerationBump
- [ ] GatewayObservedGenerationBump
- [ ] GatewayInvalidRouteKind
- [ ] GatewayInvalidTLSConfiguration
- [ ] GatewayModifyListeners
- [ ] GatewayWithAttachedRoutes
- [ ] GatewaySecretReferenceGrantAllInNamespace
- [ ] GatewaySecretReferenceGrantSpecific
- [ ] GatewaySecretInvalidReferenceGrant
- [ ] GatewaySecretMissingReferenceGrant

### Gateway Extended
- [ ] GatewayStaticAddresses
- [ ] GatewayOptionalAddressValue
- [ ] GatewayInfrastructure
- [ ] GatewayHTTPListenerIsolation
- [ ] GatewayInvalidTLSBackendConfiguration
- [ ] GatewayBackendClientCertificateFeature
- [ ] GatewayFrontendClientCertificateValidation
- [ ] GatewayFrontendClientCertificateValidationInsecureFallback
- [ ] GatewayFrontendInvalidDefaultClientCertificateValidation
- [ ] GatewayInvalidFrontendClientCertificateValidation

### HTTPRoute Core
- [ ] HTTPRouteSimpleSameNamespace
- [ ] HTTPRouteCrossNamespace
- [ ] HTTPRouteExactPathMatching
- [ ] HTTPRouteHeaderMatching
- [ ] HTTPRouteHostnameIntersection
- [ ] HTTPRouteListenerHostnameMatching
- [ ] HTTPRouteMatching
- [ ] HTTPRouteMatchingAcrossRoutes
- [ ] HTTPRouteObservedGenerationBump
- [ ] HTTPRoutePathMatchOrder
- [ ] HTTPRouteRedirectHostAndStatus
- [ ] HTTPRouteHTTPSListener
- [ ] HTTPRouteServiceTypes
- [ ] HTTPRouteWeight
- [ ] HTTPRouteDisallowedKind
- [ ] HTTPRouteInvalidBackendRefUnknownKind
- [ ] HTTPRouteInvalidCrossNamespaceBackendRef
- [ ] HTTPRouteInvalidCrossNamespaceParentRef
- [ ] HTTPRouteInvalidNonExistentBackendRef
- [ ] HTTPRouteInvalidParentRefNotMatchingSectionName
- [ ] HTTPRouteInvalidReferenceGrant
- [ ] HTTPRoutePartiallyInvalidViaInvalidReferenceGrant
- [ ] HTTPRouteReferenceGrant

### HTTPRoute Extended - 匹配
- [ ] HTTPRouteMethodMatching
- [ ] HTTPRouteQueryParamMatching

### HTTPRoute Extended - 重定向
- [ ] HTTPRouteRedirectPath
- [ ] HTTPRouteRedirectPort
- [ ] HTTPRouteRedirectPortAndScheme
- [ ] HTTPRouteRedirectScheme
- [ ] HTTPRoute303Redirect
- [ ] HTTPRoute307Redirect
- [ ] HTTPRoute308Redirect

### HTTPRoute Extended - 重写
- [ ] HTTPRouteRewriteHost
- [ ] HTTPRouteRewritePath

### HTTPRoute Extended - Header 修改
- [ ] HTTPRouteBackendRequestHeaderModifier
- [ ] HTTPRouteRequestHeaderModifierBackendWeights
- [ ] HTTPRouteResponseHeaderModifier

### HTTPRoute Extended - 镜像
- [ ] HTTPRouteRequestMirror
- [ ] HTTPRouteRequestMultipleMirrors
- [ ] HTTPRouteRequestPercentageMirror

### HTTPRoute Extended - 超时
- [ ] HTTPRouteTimeoutRequest
- [ ] HTTPRouteTimeoutBackendRequest

### HTTPRoute Extended - 协议
- [ ] HTTPRouteBackendProtocolH2C
- [ ] HTTPRouteBackendProtocolWebSocket

### HTTPRoute Extended - ParentRef 端口
- [ ] HTTPRouteListenerPortMatching
- [ ] HTTPRouteInvalidParentRefSectionNameNotMatchingPort
- [ ] HTTPRouteInvalidParentRefNotMatchingListenerPort

### HTTPRoute Extended - 其他
- [ ] HTTPRouteNamedRule
- [ ] HTTPRouteCORS
- [ ] HTTPRouteCORSAllowCredentialsBehavior
- [ ] HTTPRouteHTTPSListenerDetectMisdirectedRequests

### BackendTLSPolicy
- [ ] BackendTLSPolicy
- [ ] BackendTLSPolicyConflictResolution
- [ ] BackendTLSPolicyInvalidCACertificateRef
- [ ] BackendTLSPolicyInvalidKind
- [ ] BackendTLSPolicyObservedGenerationBump
- [ ] BackendTLSPolicySANValidation

---

## GATEWAY-TLS Profile

### TLSRoute Core (Passthrough)
- [ ] TLSRouteSimpleSameNamespace
- [ ] TLSRouteHostnameIntersection
- [ ] TLSRouteInvalidBackendRefNonexistent
- [ ] TLSRouteInvalidBackendRefUnknownKind
- [ ] TLSRouteInvalidNoMatchingListenerHostname
- [ ] TLSRouteInvalidNoMatchingListener
- [ ] TLSRouteInvalidReferenceGrant
- [ ] TLSRouteListenerPassthroughSupportedKinds

### TLSRoute Extended (Terminate)
- [ ] TLSRouteListenerTerminateNotSupported
- [ ] TLSRouteListenerTerminateSupportedKinds
- [ ] TLSRouteTerminateSimpleSameNamespace
- [ ] TLSRouteListenerMixedTerminationNotSupported

### TLSRoute Extended (Mixed)
- [ ] TLSRouteMixedTerminationSameNamespace

---

## GATEWAY-GRPC Profile

### GRPCRoute Core
- [ ] GRPCExactMethodMatching
- [ ] GRPCRouteHeaderMatching
- [ ] GRPCRouteListenerHostnameMatching
- [ ] GRPCRouteWeight

### GRPCRoute Extended
- [ ] GRPCRouteNamedRule

---

## ListenerSet (v1.5 新增)

- [ ] ListenerSetAllowedNamespaceNone
- [ ] ListenerSetAllowedNamespaceSame
- [ ] ListenerSetAllowedNamespaceSelector
- [ ] ListenerSetAllowedRoutesNamespaces
- [ ] ListenerSetAllowedRoutesSupportedKinds
- [ ] ListenerSetDefaultNotAllowed
- [ ] ListenerSetHostnameConflict
- [ ] ListenerSetHTTPRouting
- [ ] ListenerSetProtocolConflict
- [ ] ListenerSetReferenceGrant

---

## MESH-HTTP / MESH-GRPC Profile

> Edgion 暂不支持 Mesh profile，以下标记为 N/A

- [N/A] MeshBasic
- [N/A] MeshHTTPRouteSimpleSameNamespace
- [N/A] MeshHTTPRouteMatching
- [N/A] MeshHTTPRouteRedirectHostAndStatus
- [N/A] MeshHTTPRouteWeight
- [N/A] MeshTrafficSplit
- [N/A] MeshFrontend
- [N/A] MeshPorts
- [N/A] MeshHTTPRouteBackendRequestHeaderModifier
- [N/A] MeshHTTPRouteRewritePath
- [N/A] MeshHTTPRouteSchemeRedirect
- [N/A] MeshHTTPRouteRedirectPort
- [N/A] MeshHTTPRouteRedirectPath
- [N/A] MeshHTTPRouteQueryParamMatching
- [N/A] MeshHTTPRouteNamedRule
- [N/A] MeshConsumerRoute
- [N/A] MeshFrontendHostname
- [N/A] MeshGRPCRouteWeight
- [N/A] MeshHTTPRoute303Redirect
- [N/A] MeshHTTPRoute307Redirect
- [N/A] MeshHTTPRoute308Redirect

---

## UDPRoute

- [ ] UDPRoute
