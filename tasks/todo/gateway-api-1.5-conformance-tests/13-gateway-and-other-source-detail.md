# Gateway and Non-HTTPRoute Conformance Test Source Detail

> Gateway API v1.5.0 conformance tests (non-HTTPRoute) extracted from Go source.
> Covers Gateway, BackendTLSPolicy, TLSRoute, GRPCRoute, ListenerSet, UDPRoute, and Mesh tests.

**Total tests: 72**

## Table of Contents

### Gateway (21 tests)

1. [GatewayBackendClientCertificateFeature](#gatewaybackendclientcertificatefeature) (validation only)
2. [GatewayClassObservedGenerationBump](#gatewayclassobservedgenerationbump) (validation only)
3. [GatewayFrontendClientCertificateValidation](#gatewayfrontendclientcertificatevalidation) (validation only)
4. [GatewayFrontendClientCertificateValidationInsecureFallback](#gatewayfrontendclientcertificatevalidationinsecurefallback) (validation only)
5. [GatewayFrontendInvalidDefaultClientCertificateValidation](#gatewayfrontendinvaliddefaultclientcertificatevalidation) (validation only)
6. [GatewayHTTPListenerIsolation](#gatewayhttplistenerisolation) (18 cases)
7. [GatewayInfrastructure](#gatewayinfrastructure) *(Provisional)* (validation only)
8. [GatewayInvalidFrontendClientCertificateValidation](#gatewayinvalidfrontendclientcertificatevalidation) (validation only)
9. [GatewayInvalidRouteKind](#gatewayinvalidroutekind) (validation only)
10. [GatewayInvalidTLSBackendConfiguration](#gatewayinvalidtlsbackendconfiguration) (validation only)
11. [GatewayInvalidTLSConfiguration](#gatewayinvalidtlsconfiguration) (validation only)
12. [GatewayModifyListeners](#gatewaymodifylisteners) (validation only)
13. [GatewayObservedGenerationBump](#gatewayobservedgenerationbump) (validation only)
14. [GatewayOptionalAddressValue](#gatewayoptionaladdressvalue) *(Provisional)* (validation only)
15. [GatewaySecretInvalidReferenceGrant](#gatewaysecretinvalidreferencegrant) (validation only)
16. [GatewaySecretMissingReferenceGrant](#gatewaysecretmissingreferencegrant) (validation only)
17. [GatewaySecretReferenceGrantAllInNamespace](#gatewaysecretreferencegrantallinnamespace) (validation only)
18. [GatewaySecretReferenceGrantSpecific](#gatewaysecretreferencegrantspecific) (validation only)
19. [GatewayStaticAddresses](#gatewaystaticaddresses) (validation only)
20. [GatewayWithAttachedRoutes](#gatewaywithattachedroutes) (validation only)
21. [GatewayWithAttachedRoutesWithPort8080](#gatewaywithattachedrouteswithport8080) (validation only)

### BackendTLSPolicy (6 tests)

22. [BackendTLSPolicy](#backendtlspolicy) (validation only)
23. [BackendTLSPolicyConflictResolution](#backendtlspolicyconflictresolution) (validation only)
24. [BackendTLSPolicyInvalidCACertificateRef](#backendtlspolicyinvalidcacertificateref) (validation only)
25. [BackendTLSPolicyInvalidKind](#backendtlspolicyinvalidkind) (validation only)
26. [BackendTLSPolicyObservedGenerationBump](#backendtlspolicyobservedgenerationbump) (validation only)
27. [BackendTLSPolicySANValidation](#backendtlspolicysanvalidation) (validation only)

### TLSRoute (13 tests)

28. [TLSRouteHostnameIntersection](#tlsroutehostnameintersection) (validation only)
29. [TLSRouteInvalidBackendRefNonexistent](#tlsrouteinvalidbackendrefnonexistent) (validation only)
30. [TLSRouteInvalidBackendRefUnknownKind](#tlsrouteinvalidbackendrefunknownkind) (validation only)
31. [TLSRouteInvalidNoMatchingListener](#tlsrouteinvalidnomatchinglistener) (validation only)
32. [TLSRouteInvalidNoMatchingListenerHostname](#tlsrouteinvalidnomatchinglistenerhostname) (validation only)
33. [TLSRouteInvalidReferenceGrant](#tlsrouteinvalidreferencegrant) (validation only)
34. [TLSRouteListenerMixedTerminationNotSupported](#tlsroutelistenermixedterminationnotsupported) (validation only)
35. [TLSRouteListenerPassthroughSupportedKinds](#tlsroutelistenerpassthroughsupportedkinds) (validation only)
36. [TLSRouteListenerTerminateNotSupported](#tlsroutelistenerterminatenotsupported) (validation only)
37. [TLSRouteListenerTerminateSupportedKinds](#tlsroutelistenerterminatesupportedkinds) (validation only)
38. [TLSRouteMixedTerminationSameNamespace](#tlsroutemixedterminationsamenamespace) *(Provisional)* (validation only)
39. [TLSRouteSimpleSameNamespace](#tlsroutesimplesamenamespace) (validation only)
40. [TLSRouteTerminateSimpleSameNamespace](#tlsrouteterminatesimplesamenamespace) *(Provisional)* (validation only)

### GRPCRoute (6 tests)

41. [GRPCExactMethodMatching](#grpcexactmethodmatching) (3 cases)
42. [GRPCRouteHeaderMatching](#grpcrouteheadermatching) (11 cases)
43. [GRPCRouteListenerHostnameMatching](#grpcroutelistenerhostnamematching) (8 cases)
44. [GRPCRouteNamedRule](#grpcroutenamedrule) *(Provisional)* (2 cases)
45. [GRPCRouteWeight](#grpcrouteweight) (validation only)
46. [MeshGRPCRouteWeight](#meshgrpcrouteweight) (validation only)

### ListenerSet (10 tests)

47. [ListenerSetAllowedNamespaceNone](#listenersetallowednamespacenone) (validation only)
48. [ListenerSetAllowedNamespaceSame](#listenersetallowednamespacesame) (validation only)
49. [ListenerSetAllowedNamespaceSelector](#listenersetallowednamespaceselector) (validation only)
50. [ListenerSetAllowedRoutesNamespaces](#listenersetallowedroutesnamespaces) (9 cases)
51. [ListenerSetAllowedRoutesSupportedKinds](#listenersetallowedroutessupportedkinds) (validation only)
52. [ListenerSetDefaultNotAllowed](#listenersetdefaultnotallowed) (validation only)
53. [ListenerSetHTTPRouting](#listenersethttprouting) (36 cases)
54. [ListenerSetHostnameConflict](#listenersethostnameconflict) (validation only)
55. [ListenerSetProtocolConflict](#listenersetprotocolconflict) (validation only)
56. [ListenerSetReferenceGrant](#listenersetreferencegrant) (validation only)

### UDPRoute (1 tests)

57. [UDPRoute](#udproute) *(Provisional)* (validation only)

### Mesh (15 tests)

58. [MeshHTTPRoute303Redirect](#meshhttproute303redirect) *(Provisional)* (1 cases)
59. [MeshHTTPRoute307Redirect](#meshhttproute307redirect) *(Provisional)* (1 cases)
60. [MeshHTTPRoute308Redirect](#meshhttproute308redirect) *(Provisional)* (1 cases)
61. [MeshHTTPRouteBackendRequestHeaderModifier](#meshhttproutebackendrequestheadermodifier) (validation only)
62. [MeshHTTPRouteMatching](#meshhttproutematching) (9 cases)
63. [MeshHTTPRouteNamedRule](#meshhttproutenamedrule) *(Provisional)* (2 cases)
64. [MeshHTTPRouteQueryParamMatching](#meshhttproutequeryparammatching) (validation only)
65. [MeshHTTPRouteRedirectHostAndStatus](#meshhttprouteredirecthostandstatus) (2 cases)
66. [MeshHTTPRouteRedirectPath](#meshhttprouteredirectpath) (6 cases)
67. [MeshHTTPRouteRedirectPort](#meshhttprouteredirectport) (4 cases)
68. [MeshHTTPRouteRequestHeaderModifier](#meshhttprouterequestheadermodifier) (7 cases)
69. [MeshHTTPRouteRewritePath](#meshhttprouterewritepath) (validation only)
70. [MeshHTTPRouteSchemeRedirect](#meshhttprouteschemeredirect) (4 cases)
71. [MeshHTTPRouteSimpleSameNamespace](#meshhttproutesimplesamenamespace) (validation only)
72. [MeshHTTPRouteWeight](#meshhttprouteweight) (validation only)

---

## BackendTLSPolicy

- **Description:** BackendTLSPolicy must be used to configure TLS connection between gateway and backend
- **Manifests:** `tests/backendtlspolicy.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportBackendTLSPolicy`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `backendtlspolicy-reencrypt` |
| Gateway | `gwNN` | `same-namespace-with-https-listener` |
| Resource | `validPolicyNN` | `normative-test` |
| Resource | `certNN` | `tls-validity-checks-certificate` |
| Route | `routeNN` | `backendtlspolicy` |
| Gateway | `gwNN` | `same-namespace` |
| Resource | `validPolicyNN` | `normative-test` |
| Resource | `invalidPolicyNN` | `host-mismatch` |
| Resource | `invalidCertPolicyNN` | `cert-mismatch` |
| Gateway | `gwNN` | `same-namespace` |
| Resource | `testPolicyNN` | `reconcile-test` |
| Resource | `sharedCMNN` | `tls-checks-ca-certificate` |

### Preconditions

- Gateway and Routes must be accepted
- Route must have ResolvedRefs=True
- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Makes TLS request and validates response
- Tests TLS to backend

---

## BackendTLSPolicyConflictResolution

- **Description:** Verifies that when multiple BackendTLSPolicies target the same Service, only one policy is accepted while conflicting policies are rejected, and traffic continues to route successfully.
- **Manifests:** `tests/backendtlspolicy-conflict-resolution.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportBackendTLSPolicy`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `backendtlspolicy-conflict-resolution` |
| Gateway | `gwNN` | `same-namespace` |
| Resource | `policyNN` | `conflicted-without-section-name-1` |
| Resource | `conflictedPolicyNN` | `conflicted-without-section-name-2` |
| Resource | `policyNN` | `conflicted-with-section-name-1` |
| Resource | `conflictedPolicyNN` | `conflicted-with-section-name-2` |
| Resource | `policyNN` | `not-conflicted-with-section-name` |
| Resource | `policyNN` | `not-conflicted-without-section-name` |

### Preconditions

- Gateway and Routes must be accepted
- Route must have ResolvedRefs=True
- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Tests TLS to backend

---

## BackendTLSPolicyInvalidCACertificateRef

- **Description:** A BackendTLSPolicy that specifies a single invalid CACertificateRef should have the Accepted and ResolvedRefs status condition set False with appropriate reasons, and HTTP requests to a backend targeted by this policy should fail with a 5xx response.
- **Manifests:** `tests/backendtlspolicy-invalid-ca-certificate-ref.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportBackendTLSPolicy`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `backendtlspolicy-invalid-ca-certificate-ref` |
| Gateway | `gwNN` | `same-namespace` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Tests TLS to backend

---

## BackendTLSPolicyInvalidKind

- **Description:** A BackendTLSPolicy that specifies a single CACertificateRef with an invalid kind should have the Accepted and ResolvedRefs status condition set False with appropriate reasons, and HTTP requests to a backend targeted by this policy should fail with a 5xx response.
- **Manifests:** `tests/backendtlspolicy-invalid-kind.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportBackendTLSPolicy`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `backendtlspolicy-invalid-kind-test` |
| Gateway | `gwNN` | `same-namespace` |
| Resource | `policyNN` | `invalid-kind` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Tests TLS to backend

---

## BackendTLSPolicyObservedGenerationBump

- **Description:** A BackendTLSPolicy in the gateway-conformance-infra namespace should update the observedGeneration in all of its Status.Conditions after an update to the spec
- **Manifests:** `tests/backendtlspolicy-observed-generation-bump.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportBackendTLSPolicy`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Resource | `policyNN` | `observed-generation-bump` |
| Gateway | `gwNN` | `same-namespace` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates ObservedGeneration bump
- Tests TLS to backend

---

## BackendTLSPolicySANValidation

- **Description:** BackendTLSPolicySANValidation extend BackendTLSPolicy with SubjectAltNames validation
- **Manifests:** `tests/backendtlspolicy-san.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportBackendTLSPolicy`, `SupportBackendTLSPolicySANValidation`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `backendtlspolicy-san-test` |
| Gateway | `gwNN` | `same-namespace` |
| Resource | `policyNN` | `san-dns` |
| Resource | `policyNN` | `san-dns-mismatch` |
| Resource | `policyNN` | `san-uri` |
| Resource | `policyNN` | `san-uri-mismatch` |
| Resource | `policyNN` | `multiple-sans` |
| Resource | `policyNN` | `multiple-mismatch-sans` |

### Preconditions

- Gateway and Routes must be accepted
- Route must have ResolvedRefs=True
- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Tests TLS to backend

---

## GRPCExactMethodMatching

- **Description:** A single GRPCRoute with exact method matching for different backends
- **Manifests:** `tests/grpcroute-exact-method-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGRPCRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `exact-matching` |
| Gateway | `gwNN` | `same-namespace` |

### Preconditions

- Gateway and Routes must be accepted

### gRPC Test Cases (3)

| # | Service | Method | Backend |
|---|---------|--------|---------|
| 1 | `` | `` | `grpc-infra-backend-v1` |
| 2 | `` | `` | `grpc-infra-backend-v2` |
| 3 | `` | `` | `` |

---

## GRPCRouteHeaderMatching

- **Description:** A single GRPCRoute with header matching for different backends
- **Manifests:** `tests/grpcroute-header-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGRPCRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `grpc-header-matching` |
| Gateway | `gwNN` | `same-namespace` |

### Preconditions

- Gateway and Routes must be accepted

### gRPC Test Cases (11)

| # | Service | Method | Backend |
|---|---------|--------|---------|
| 1 | `` | `` | `grpc-infra-backend-v1` |
| 2 | `` | `` | `grpc-infra-backend-v2` |
| 3 | `` | `` | `grpc-infra-backend-v1` |
| 4 | `` | `` | `grpc-infra-backend-v2` |
| 5 | `` | `` | `` |
| 6 | `` | `` | `` |
| 7 | `` | `` | `grpc-infra-backend-v1` |
| 8 | `` | `` | `grpc-infra-backend-v1` |
| 9 | `` | `` | `grpc-infra-backend-v2` |
| 10 | `` | `` | `grpc-infra-backend-v2` |
| 11 | `` | `` | `` |

---

## GRPCRouteListenerHostnameMatching

- **Description:** Multiple GRPC listeners with the same port and different hostnames, each with a different GRPCRoute
- **Manifests:** `tests/grpcroute-listener-hostname-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGRPCRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `grpcroute-listener-hostname-matching` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready

### gRPC Test Cases (8)

| # | Service | Method | Backend |
|---|---------|--------|---------|
| 1 | `` | `` | `grpc-infra-backend-v1` |
| 2 | `` | `` | `grpc-infra-backend-v2` |
| 3 | `` | `` | `grpc-infra-backend-v3` |
| 4 | `` | `` | `grpc-infra-backend-v3` |
| 5 | `` | `` | `grpc-infra-backend-v3` |
| 6 | `` | `` | `grpc-infra-backend-v3` |
| 7 | `` | `` | `` |
| 8 | `` | `` | `` |

---

## GRPCRouteNamedRule

- **Description:** A GRPCRoute with a named GRPCRouteRule
- **Status:** Provisional
- **Manifests:** `tests/grpcroute-named-rule.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGRPCRoute`, `SupportGRPCRouteNamedRouteRule`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `grpc-named-rules` |
| Gateway | `gwNN` | `same-namespace` |

### Preconditions

- Gateway and Routes must be accepted

### gRPC Test Cases (2)

| # | Service | Method | Backend |
|---|---------|--------|---------|
| 1 | `` | `` | `grpc-infra-backend-v1` |
| 2 | `` | `` | `grpc-infra-backend-v2` |

---

## GRPCRouteWeight

- **Description:** A GRPCRoute with weighted backends
- **Manifests:** `tests/grpcroute-weight.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGRPCRoute`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Gateway and Routes must be accepted

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewayBackendClientCertificateFeature

- **Description:** A Gateway with a client certificate configured should present the certificate when connecting to a backend using TLS.
- **Manifests:** `tests/gateway-tls-backend-client-certificate.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGatewayBackendClientCertificate`, `SupportHTTPRoute`, `SupportBackendTLSPolicy`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `gateway-tls-backend-client-certificate` |
| Gateway | `gwNN` | `gateway-tls-backend-client-certificate` |
| Resource | `policyNN` | `gateway-tls-backend-client-certificate-test` |

### Preconditions

- Gateway and Routes must be accepted
- Route must have ResolvedRefs=True
- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions
- Tests mTLS / client certificate

---

## GatewayClassObservedGenerationBump

- **Description:** A GatewayClass should update the observedGeneration in all of it's Status.Conditions after an update to the spec
- **Manifests:** `tests/gatewayclass-observed-generation-bump.yaml`
- **Features:** `FeatureName`, `SupportGateway`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwcNN` | `gatewayclass-observed-generation-bump` |

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates ObservedGeneration bump

---

## GatewayFrontendClientCertificateValidation

- **Description:** Gateway's client certificate validation config should be used for HTTPS traffic
- **Manifests:** `tests/gateway-with-clientcertificate-validation.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportGatewayFrontendClientCertificateValidation`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `client-validation-default` |
| Resource | `certNN` | `tls-validity-checks-certificate` |
| Resource | `clientCertNN` | `tls-validity-checks-client-certificate` |
| Resource | `clientCertPerPortNN` | `tls-validity-checks-per-port-client-certificate` |

### Preconditions

- Gateway and Routes must be accepted
- Route must have ResolvedRefs=True

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Makes TLS request and validates response
- Tests mTLS / client certificate

---

## GatewayFrontendClientCertificateValidationInsecureFallback

- **Description:** Gateway's client certificate validation set to AllowInsecureFallback allows any HTTPS traffic.
- **Manifests:** `tests/gateway-with-clientcertificate-validation-insecure-fallback.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportGatewayFrontendClientCertificateValidation`, `SupportGatewayFrontendClientCertificateValidationInsecureFallback`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `client-validation-insecure-fallback` |
| Resource | `certNN` | `tls-validity-checks-certificate` |
| Resource | `clientCertNN` | `tls-validity-checks-client-certificate` |
| Resource | `clientCertPerPortNN` | `tls-validity-checks-per-port-client-certificate` |

### Preconditions

- Gateway and Routes must be accepted
- Route must have ResolvedRefs=True
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Makes TLS request and validates response
- Validates Gateway conditions
- Tests mTLS / client certificate

---

## GatewayFrontendInvalidDefaultClientCertificateValidation

- **Description:** Invalid Gateway's default Client Certificate Validation Config should only affect HTTPS traffic
- **Manifests:** `tests/gateway-invalid-default-frontend-client-certificate-validation.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportGatewayFrontendClientCertificateValidation`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `invalid-default-client-validation-config` |
| Resource | `certNN` | `tls-validity-checks-certificate` |

### Preconditions

- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Makes TLS request and validates response
- Tests mTLS / client certificate

---

## GatewayHTTPListenerIsolation

- **Description:** Listener isolation for HTTP listeners with multiple listeners and HTTPRoutes
- **Manifests:** `tests/gateway-http-listener-isolation.yaml`, `tests/gateway-http-listener-isolation-with-hostname-intersection.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGatewayHTTPListenerIsolation`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `http-listener-isolation` |
| Gateway | `gwNN` | `http-listener-isolation-with-hostname-intersection` |

### Preconditions

- Gateway and Routes must be accepted
- Route must have ResolvedRefs=True
- Namespaces must be ready

### HTTP Test Cases (18)

#### Case 1: `GET /empty-hostname`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/empty-hostname` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 2: `GET /wildcard-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/wildcard-example-com` |
| Expected Status | `404` |

#### Case 3: `GET /wildcard-foo-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/wildcard-foo-example-com` |
| Expected Status | `404` |

#### Case 4: `GET /abc-foo-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/abc-foo-example-com` |
| Expected Status | `404` |

#### Case 5: `GET /empty-hostname`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/empty-hostname` |
| Expected Status | `404` |

#### Case 6: `GET /wildcard-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/wildcard-example-com` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 7: `GET /wildcard-foo-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/wildcard-foo-example-com` |
| Expected Status | `404` |

#### Case 8: `GET /abc-foo-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/abc-foo-example-com` |
| Expected Status | `404` |

#### Case 9: `GET /empty-hostname`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/empty-hostname` |
| Expected Status | `404` |

#### Case 10: `GET /wildcard-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/wildcard-example-com` |
| Expected Status | `404` |

#### Case 11: `GET /wildcard-foo-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/wildcard-foo-example-com` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 12: `GET /abc-foo-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/abc-foo-example-com` |
| Expected Status | `404` |

#### Case 13: `GET /empty-hostname`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/empty-hostname` |
| Expected Status | `404` |

#### Case 14: `GET /wildcard-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/wildcard-example-com` |
| Expected Status | `404` |

#### Case 15: `GET /wildcard-foo-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/wildcard-foo-example-com` |
| Expected Status | `404` |

#### Case 16: `GET /abc-foo-example-com`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/abc-foo-example-com` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 17: `GET `

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `` |
| Expected Status | `200` |

#### Case 18: `GET `

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `` |
| Expected Status | `200` |

---

## GatewayInfrastructure

- **Description:** Propagation of metadata from Gateway infrastructure to generated components
- **Status:** Provisional
- **Manifests:** `tests/gateway-infrastructure.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGatewayInfrastructurePropagation`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## GatewayInvalidFrontendClientCertificateValidation

- **Description:** Gateway's should reject invalid Client Certificate Validation Config
- **Manifests:** `tests/gateway-with-invalid-clientcertificate-validation.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportReferenceGrant`, `SupportGatewayFrontendClientCertificateValidation`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-invalid-client-cert-validation` |

### Preconditions

- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Tests mTLS / client certificate

---

## GatewayInvalidRouteKind

- **Description:** A Gateway in the gateway-conformance-infra namespace should fail to become ready an invalid Route kind is specified.
- **Manifests:** `tests/gateway-invalid-route-kind.yaml`
- **Features:** `FeatureName`, `SupportGateway`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-only-invalid-route-kind` |
| Gateway | `gwNN` | `gateway-supported-and-invalid-route-kind` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewayInvalidTLSBackendConfiguration

- **Description:** A Gateway should have ResolvedRefs condition set false if the Gateway has an invalid backend TLS configuration
- **Manifests:** `tests/gateway-invalid-tls-backend-configuration.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGatewayBackendClientCertificate`, `SupportHTTPRoute`, `SupportBackendTLSPolicy`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## GatewayInvalidTLSConfiguration

- **Description:** A Gateway should fail to become ready if the Gateway has an invalid TLS configuration
- **Manifests:** `tests/gateway-invalid-tls-configuration.yaml`
- **Features:** `FeatureName`, `SupportGateway`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewayModifyListeners

- **Description:** A Gateway in the gateway-conformance-infra namespace should handle adding and removing listeners.
- **Manifests:** `tests/gateway-modify-listeners.yaml`
- **Features:** `FeatureName`, `SupportGateway`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-add-listener` |
| Gateway | `gwNN` | `gateway-remove-listener` |

### Preconditions

- Namespaces must be ready
- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewayObservedGenerationBump

- **Description:** A Gateway in the gateway-conformance-infra namespace should update the observedGeneration in all of its Status.Conditions after an update to the spec
- **Manifests:** `tests/gateway-observed-generation-bump.yaml`
- **Features:** `FeatureName`, `SupportGateway`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-observed-generation-bump` |

### Preconditions

- Namespaces must be ready

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates ObservedGeneration bump

---

## GatewayOptionalAddressValue

- **Description:** Check Gateway Support for GatewayAddressEmpty feature
- **Status:** Provisional
- **Manifests:** `tests/gateway-optional-address-value.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGatewayAddressEmpty`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Namespaces must be ready

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewaySecretInvalidReferenceGrant

- **Description:** A Gateway in the gateway-conformance-infra namespace should fail to become ready if the Gateway has a certificateRef for a Secret in the gateway-conformance-web-backend namespace and a ReferenceGrant exists but does not grant permission to that specific Secret
- **Manifests:** `tests/gateway-secret-invalid-reference-grant.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-secret-invalid-reference-grant` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewaySecretMissingReferenceGrant

- **Description:** A Gateway in the gateway-conformance-infra namespace should fail to become programmed if the Gateway has a certificateRef for a Secret in the gateway-conformance-web-backend namespace and a ReferenceGrant granting permission to the Secret does not exist
- **Manifests:** `tests/gateway-secret-missing-reference-grant.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-secret-missing-reference-grant` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewaySecretReferenceGrantAllInNamespace

- **Description:** A Gateway in the gateway-conformance-infra namespace should become programmed if the Gateway has a certificateRef for a Secret in the gateway-conformance-web-backend namespace and a ReferenceGrant granting permission to all Secrets in the namespace exists
- **Manifests:** `tests/gateway-secret-reference-grant-all-in-namespace.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-secret-reference-grant-all-in-namespace` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewaySecretReferenceGrantSpecific

- **Description:** A Gateway in the gateway-conformance-infra namespace should become programmed if the Gateway has a certificateRef for a Secret in the gateway-conformance-web-backend namespace and a ReferenceGrant granting permission to the specific Secret exists
- **Manifests:** `tests/gateway-secret-reference-grant-specific.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-secret-reference-grant-specific` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewayStaticAddresses

- **Description:** A Gateway in the gateway-conformance-infra namespace should be able to use previously determined addresses.
- **Manifests:** `tests/gateway-static-addresses.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGatewayStaticAddresses`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Gateway must have expected listeners
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## GatewayWithAttachedRoutes

- **Description:** A Gateway in the gateway-conformance-infra namespace should be attached to routes.
- **Manifests:** `tests/gateway-with-attached-routes.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-one-attached-route` |
| Gateway | `gwNN` | `gateway-with-two-attached-routes` |
| Route | `hrouteNA` | `http-route-not-accepted` |
| Gateway | `gwNN` | `unresolved-gateway-with-one-attached-unresolved-route` |
| Route | `hrouteNN` | `http-route-4` |

### Preconditions

- Gateway must have expected listeners
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## GatewayWithAttachedRoutesWithPort8080

- **Description:** A Gateway in the gateway-conformance-infra namespace should be attached to routes.
- **Manifests:** `tests/gateway-with-attached-routes-with-port-8080.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGatewayPort8080`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-two-listeners-and-one-attached-route` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## ListenerSetAllowedNamespaceNone

- **Description:** Listener Sets are not allowed on the Gateway when `allowedListeners` is set to `None`
- **Manifests:** `tests/listenerset-allowed-namespace-none.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-does-not-allow-listenerset` |
| Resource | `disallowedLsNN` | `listenerset-not-allowed` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## ListenerSetAllowedNamespaceSame

- **Description:** ListenerSets in the same namespace as the Gateway are allowed when `allowedListeners` is set to `Same`
- **Manifests:** `tests/listenerset-allowed-namespace-same.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-allows-listenerset-in-same-namespace` |
| Resource | `lsNN` | `listenerset-in-same-namespace` |
| Resource | `disallowedLsNN` | `listenerset-in-different-namespace` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions
- Listeners must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## ListenerSetAllowedNamespaceSelector

- **Description:** ListenerSets in the namespaces that match the labels are allowed when `allowedListeners` is set to `Selector`
- **Manifests:** `tests/listenerset-allowed-namespace-selector.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-allows-listenerset-in-selected-namespace` |
| Resource | `lsNN` | `listenerset-in-selected-namespace` |
| Resource | `disallowedLsNN` | `listenerset-not-in-selected-namespace` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions
- Listeners must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## ListenerSetAllowedRoutesNamespaces

- **Description:** ListenerSet listeners allow routes from the specified namespace
- **Manifests:** `tests/listenerset-allowed-routes-namespaces.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`, `SupportHTTPRoute`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-listener-sets-test-allowed-routes` |
| Resource | `lsNN` | `listenerset-test-allowed-routes-namespaces` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready
- Resources must have expected conditions

### HTTP Test Cases (9)

#### Case 1: `GET /route-in-same-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-in-same-namespace` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 2: `GET /route-in-selected-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-in-selected-namespace` |
| Expected Status | `200` |
| Backend | `infra-backend-v2` |

#### Case 3: `GET /route-not-in-selected-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-not-in-selected-namespace` |
| Expected Status | `200` |
| Backend | `infra-backend-v3` |

#### Case 4: `GET /route-in-same-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-in-same-namespace` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 5: `GET /route-in-selected-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-in-selected-namespace` |
| Expected Status | `404` |

#### Case 6: `GET /route-not-in-selected-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-not-in-selected-namespace` |
| Expected Status | `404` |

#### Case 7: `GET /route-in-same-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-in-same-namespace` |
| Expected Status | `404` |

#### Case 8: `GET /route-in-selected-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-in-selected-namespace` |
| Expected Status | `200` |
| Backend | `infra-backend-v2` |

#### Case 9: `GET /route-not-in-selected-namespace`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route-not-in-selected-namespace` |
| Expected Status | `404` |

### Special Validations

- Validates Gateway conditions

---

## ListenerSetAllowedRoutesSupportedKinds

- **Description:** ListenerSet listeners allow specific route kinds
- **Manifests:** `tests/listenerset-allowed-routes-supported-kinds.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-listener-sets-test-supported-route-kinds` |
| Resource | `lsNN` | `listenerset-test-allowed-routes-supported-kinds` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions
- Listeners must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## ListenerSetDefaultNotAllowed

- **Description:** Listener Sets are not allowed on the Gateway by default (i.e. when `allowedListeners` is not set)
- **Manifests:** `tests/listenerset-default-not-allowed.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-default-does-not-allow-listenerset` |
| Resource | `disallowedLsNN` | `listenerset-default-not-allowed` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## ListenerSetHTTPRouting

- **Description:** HTTP Routing works as expected with ListenerSets
- **Manifests:** `tests/listenerset-http-routing.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-listener-sets-http-routing` |
| Resource | `lsNN` | `listener-set-http-routing-1` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready

### HTTP Test Cases (36)

#### Case 1: `GET /route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 2: `GET /route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 3: `GET /route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 4: `GET /route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 5: `GET /route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 6: `GET /route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/route` |
| Expected Status | `200` |
| Backend | `infra-backend-v1` |

#### Case 7: `GET /gateway-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-route` |
| Expected Status | `200` |
| Backend | `infra-backend-v2` |

#### Case 8: `GET /gateway-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-route` |
| Expected Status | `200` |
| Backend | `infra-backend-v2` |

#### Case 9: `GET /gateway-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-route` |
| Expected Status | `404` |

#### Case 10: `GET /gateway-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-route` |
| Expected Status | `404` |

#### Case 11: `GET /gateway-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-route` |
| Expected Status | `404` |

#### Case 12: `GET /gateway-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-route` |
| Expected Status | `404` |

#### Case 13: `GET /gateway-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-section-route` |
| Expected Status | `200` |
| Backend | `infra-backend-v3` |

#### Case 14: `GET /gateway-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-section-route` |
| Expected Status | `404` |

#### Case 15: `GET /gateway-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-section-route` |
| Expected Status | `404` |

#### Case 16: `GET /gateway-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-section-route` |
| Expected Status | `404` |

#### Case 17: `GET /gateway-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-section-route` |
| Expected Status | `404` |

#### Case 18: `GET /gateway-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/gateway-section-route` |
| Expected Status | `404` |

#### Case 19: `GET /listener-set-http-routing-1-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-route` |
| Expected Status | `404` |

#### Case 20: `GET /listener-set-http-routing-1-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-route` |
| Expected Status | `404` |

#### Case 21: `GET /listener-set-http-routing-1-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-route` |
| Expected Status | `200` |
| Backend | `infra-backend-v2` |

#### Case 22: `GET /listener-set-http-routing-1-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-route` |
| Expected Status | `200` |
| Backend | `infra-backend-v2` |

#### Case 23: `GET /listener-set-http-routing-1-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-route` |
| Expected Status | `404` |

#### Case 24: `GET /listener-set-http-routing-1-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-route` |
| Expected Status | `404` |

#### Case 25: `GET /listener-set-http-routing-1-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-section-route` |
| Expected Status | `404` |

#### Case 26: `GET /listener-set-http-routing-1-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-section-route` |
| Expected Status | `404` |

#### Case 27: `GET /listener-set-http-routing-1-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-section-route` |
| Expected Status | `200` |
| Backend | `infra-backend-v3` |

#### Case 28: `GET /listener-set-http-routing-1-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-section-route` |
| Expected Status | `404` |

#### Case 29: `GET /listener-set-http-routing-1-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-section-route` |
| Expected Status | `404` |

#### Case 30: `GET /listener-set-http-routing-1-section-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-1-section-route` |
| Expected Status | `404` |

#### Case 31: `GET /listener-set-http-routing-2-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-2-route` |
| Expected Status | `404` |

#### Case 32: `GET /listener-set-http-routing-2-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-2-route` |
| Expected Status | `404` |

#### Case 33: `GET /listener-set-http-routing-2-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-2-route` |
| Expected Status | `404` |

#### Case 34: `GET /listener-set-http-routing-2-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-2-route` |
| Expected Status | `404` |

#### Case 35: `GET /listener-set-http-routing-2-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-2-route` |
| Expected Status | `200` |
| Backend | `infra-backend-v2` |

#### Case 36: `GET /listener-set-http-routing-2-route`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/listener-set-http-routing-2-route` |
| Expected Status | `200` |
| Backend | `infra-backend-v2` |

---

## ListenerSetHostnameConflict

- **Description:** Validate Listener Precedence when a ListenerSet listener has a hostname conflict
- **Manifests:** `tests/listenerset-hostname-conflict.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-listenerset-hostname-conflict` |
| Resource | `lsNN` | `listenerset-with-hostname-conflict-with-gateway-1` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions
- Listeners must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## ListenerSetProtocolConflict

- **Description:** Validate Listener Precedence when a ListenerSet listener has a protocol conflict
- **Manifests:** `tests/listenerset-protocol-conflict.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-listenerset-protocol-conflict` |
| Resource | `lsNN` | `listenerset-with-protocol-conflict-with-gateway-1` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions
- Listeners must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## ListenerSetReferenceGrant

- **Description:** ListenerSet with reference grant support
- **Manifests:** `tests/listenerset-reference-grant.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportListenerSet`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-with-listener-sets-test-reference-grant` |
| Resource | `lsNN` | `listenerset-with-reference-grant` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions
- Listeners must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Validates Gateway conditions

---

## MeshGRPCRouteWeight

- **Description:** A GRPCRoute with weighted backends in mesh mode
- **Manifests:** `tests/mesh/grpcroute-weight.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportGRPCRoute`
- **Namespace:** `gateway-conformance-infra`

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## MeshHTTPRoute303Redirect

- **Description:** An HTTPRoute with statusCode 303 redirect filter
- **Status:** Provisional
- **Manifests:** `tests/mesh/httproute-303-redirect.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportHTTPRoute303RedirectStatusCode`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (1)

#### Case 1: `GET /redirect`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/redirect` |
| Expected Status | `303` |

---

## MeshHTTPRoute307Redirect

- **Description:** An HTTPRoute with statusCode 307 redirect filter
- **Status:** Provisional
- **Manifests:** `tests/mesh/httproute-303-redirect.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportHTTPRoute307RedirectStatusCode`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (1)

#### Case 1: `GET /temporary`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/temporary` |
| Expected Status | `307` |

---

## MeshHTTPRoute308Redirect

- **Description:** An HTTPRoute with statusCode 308 redirect filter
- **Status:** Provisional
- **Manifests:** `tests/mesh/httproute-308-redirect.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportHTTPRoute308RedirectStatusCode`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (1)

#### Case 1: `GET /permanent`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/permanent` |
| Expected Status | `308` |

---

## MeshHTTPRouteBackendRequestHeaderModifier

- **Description:** An HTTPRoute backend has request header modifier filters applied correctly
- **Manifests:** `tests/mesh/httproute-request-header-modifier-backend.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportMeshHTTPRouteBackendRequestHeaderModification`
- **Namespace:** `gateway-conformance-infra`

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## MeshHTTPRouteMatching

- **Description:** A single HTTPRoute with path and header matching for different backends
- **Manifests:** `tests/mesh/httproute-matching.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (9)

#### Case 1: `GET /`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 2: `GET /example`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/example` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 3: `GET /`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Request Header `Version` | `one` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 4: `GET /v2`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2` |
| Expected Status | `200` |
| Backend | `echo-v2` |

#### Case 5: `GET /v2/example`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2/example` |
| Expected Status | `200` |
| Backend | `echo-v2` |

#### Case 6: `GET /`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Request Header `Version` | `two` |
| Expected Status | `200` |
| Backend | `echo-v2` |

#### Case 7: `GET /v2/`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2/` |
| Expected Status | `200` |
| Backend | `echo-v2` |

#### Case 8: `GET /v2example`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2example` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 9: `GET /foo/v2/example`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/foo/v2/example` |
| Expected Status | `200` |
| Backend | `echo-v1` |

---

## MeshHTTPRouteNamedRule

- **Description:** An HTTPRoute with a named HTTPRouteRule
- **Status:** Provisional
- **Manifests:** `tests/mesh/httproute-named-rule.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportMeshHTTPRouteNamedRouteRule`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (2)

#### Case 1: `GET /named`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/named` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 2: `GET /unnamed`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/unnamed` |
| Expected Status | `200` |
| Backend | `echo-v2` |

---

## MeshHTTPRouteQueryParamMatching

- **Description:** A single HTTPRoute with query param matching for different backends
- **Manifests:** `tests/mesh/httproute-query-param-matching.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportMeshHTTPRouteQueryParamMatching`
- **Namespace:** `gateway-conformance-mesh`

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## MeshHTTPRouteRedirectHostAndStatus

- **Description:** An HTTPRoute with hostname and statusCode redirect filters
- **Manifests:** `tests/mesh/httproute-redirect-host-and-status.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (2)

#### Case 1: `GET /hostname-redirect`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/hostname-redirect` |
| Expected Status | `302` |

#### Case 2: `GET /host-and-status`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/host-and-status` |
| Expected Status | `301` |

---

## MeshHTTPRouteRedirectPath

- **Description:** An HTTPRoute with scheme redirect filter
- **Manifests:** `tests/mesh/httproute-redirect-path.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportMeshHTTPRouteRedirectPath`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (6)

#### Case 1: `GET /original-prefix/lemon`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/original-prefix/lemon` |
| Expected Status | `302` |

#### Case 2: `GET /full/path/original`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/full/path/original` |
| Expected Status | `302` |

#### Case 3: `GET /path-and-host`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/path-and-host` |
| Expected Status | `302` |

#### Case 4: `GET /path-and-status`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/path-and-status` |
| Expected Status | `301` |

#### Case 5: `GET /full-path-and-host`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/full-path-and-host` |
| Expected Status | `302` |

#### Case 6: `GET /full-path-and-status`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/full-path-and-status` |
| Expected Status | `301` |

---

## MeshHTTPRouteRedirectPort

- **Description:** An HTTPRoute with a port redirect filter
- **Manifests:** `tests/mesh/httproute-redirect-port.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportMeshHTTPRouteRedirectPort`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (4)

#### Case 1: `GET /port`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/port` |
| Expected Status | `302` |

#### Case 2: `GET /port-and-host`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/port-and-host` |
| Expected Status | `302` |

#### Case 3: `GET /port-and-status`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/port-and-status` |
| Expected Status | `301` |

#### Case 4: `GET /port-and-host-and-status`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/port-and-host-and-status` |
| Expected Status | `302` |

---

## MeshHTTPRouteRequestHeaderModifier

- **Description:** An HTTPRoute has request header modifier filters applied correctly
- **Manifests:** `tests/mesh/httproute-request-header-modifier.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (7)

#### Case 1: `GET /set`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/set` |
| Request Header `Some-Other-Header` | `val` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 2: `GET /set`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/set` |
| Request Header `Some-Other-Header` | `val` |
| Request Header `X-Header-Set` | `some-other-value` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 3: `GET /add`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/add` |
| Request Header `Some-Other-Header` | `val` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 4: `GET /add`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/add` |
| Request Header `Some-Other-Header` | `val` |
| Request Header `X-Header-Add` | `some-other-value` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 5: `GET /remove`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/remove` |
| Request Header `X-Header-Remove` | `val` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 6: `GET /multiple`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/multiple` |
| Request Header `X-Header-Set-2` | `set-val-2` |
| Request Header `X-Header-Add-2` | `add-val-2` |
| Request Header `X-Header-Remove-2` | `remove-val-2` |
| Request Header `Another-Header` | `another-header-val` |
| Expected Status | `200` |
| Backend | `echo-v1` |

#### Case 7: `GET /case-insensitivity`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/case-insensitivity` |
| Request Header `x-header-set` | `original-val-set` |
| Request Header `x-header-add` | `original-val-add` |
| Request Header `x-header-remove` | `original-val-remove` |
| Request Header `Another-Header` | `another-header-val` |
| Expected Status | `200` |
| Backend | `echo-v1` |

---

## MeshHTTPRouteRewritePath

- **Description:** An HTTPRoute with path rewrite filter
- **Manifests:** `tests/mesh/httproute-rewrite-path.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportMeshHTTPRouteRewritePath`
- **Namespace:** `gateway-conformance-mesh`

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## MeshHTTPRouteSchemeRedirect

- **Description:** An HTTPRoute with a scheme redirect filter
- **Manifests:** `tests/mesh/httproute-redirect-scheme.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`, `SupportMeshHTTPRouteSchemeRedirect`
- **Namespace:** `gateway-conformance-mesh`

### HTTP Test Cases (4)

#### Case 1: `GET /scheme`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme` |
| Expected Status | `302` |

#### Case 2: `GET /scheme-and-host`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-and-host` |
| Expected Status | `302` |

#### Case 3: `GET /scheme-and-status`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-and-status` |
| Expected Status | `301` |

#### Case 4: `GET /scheme-and-host-and-status`

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-and-host-and-status` |
| Expected Status | `302` |

---

## MeshHTTPRouteSimpleSameNamespace

- **Description:** A single HTTPRoute in the gateway-conformance-mesh namespace attaches to a Service in the same namespace
- **Manifests:** `tests/mesh/httproute-simple-same-namespace.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-mesh`

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## MeshHTTPRouteWeight

- **Description:** An HTTPRoute with weighted backends
- **Manifests:** `tests/mesh/httproute-weight.yaml`
- **Features:** `FeatureName`, `SupportMesh`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteHostnameIntersection

- **Description:** TLSRoutes should attach to listeners only if they have intersecting hostnames, and should accept requests only for the intersecting hostnames
- **Manifests:** `tests/tlsroute-hostname-intersection.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Resource | `certNN` | `tls-checks-certificate` |
| Gateway | `gwNN` | `gw-tlsroute-exact-hostname-x-1` |
| Gateway | `gwNN` | `gw-tlsroute-more-specific-wc-hostname-x-2` |
| Gateway | `gwNN` | `gw-tlsroute-less-specific-wc-hostname-x-3` |
| Gateway | `gwNN` | `gw-tlsroute-empty-hostname-x-4` |

### Preconditions

- Gateway and Routes must be accepted
- Route must have ResolvedRefs=True
- Namespaces must be ready

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Makes TLS request and validates response

---

## TLSRouteInvalidBackendRefNonexistent

- **Description:** A single TLSRoute in the gateway-conformance-infra namespace has a ResolvedRefs status False with reason BackendNotFound when binding to a Gateway in the same namespace, if the route has a BackendRef Service that does not exist
- **Manifests:** `tests/tlsroute-invalid-backendref-nonexistent.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `invalid-backend-ref-nonexistent` |
| Gateway | `gwNN` | `gateway-tlsroute-invalid-backend-ref-nonexistent` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteInvalidBackendRefUnknownKind

- **Description:** A single TLSRoute in the gateway-conformance-infra namespace should set a ResolvedRefs status False with reason InvalidKind when attempting to bind to a Gateway in the same namespace if the route has a BackendRef that points to an unknown Kind.
- **Manifests:** `tests/tlsroute-invalid-backendref-unknown-kind.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `invalid-backend-ref-unknown-kind` |
| Gateway | `gwNN` | `gateway-tlsroute-invalid-backend-ref-unknown-kind` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteInvalidNoMatchingListener

- **Description:** A TLSRoute should set Accepted=False when attaching to a Gateway with no compatible TLS listener
- **Manifests:** `tests/tlsroute-invalid-no-matching-listener.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNotAllowedProtocolHTTPNN` | `tlsroute-not-allowed-protocol-http` |
| Route | `routeNotAllowedProtocolHTTPSNN` | `tlsroute-not-allowed-protocol-https` |
| Route | `routeNoMatchingSectionNN` | `tlsroute-no-matching-section-name` |
| Gateway | `gwTLSPassthroughOnlyNN` | `gateway-tlsroute-tls-passthrough-only` |
| Gateway | `gwHTTPOnlyNN` | `gateway-tlsroute-http-only` |
| Gateway | `gwHTTPSOnlyNN` | `gateway-tlsroute-https-only` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteInvalidNoMatchingListenerHostname

- **Description:** A TLSRoute with a hostname that does not match the Gateway listener hostname should set Accepted=False with Reason=NoMatchingListenerHostname
- **Manifests:** `tests/tlsroute-invalid-no-matching-listener-hostname.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `route1NN` | `tlsroute-hostname-mismatch-1` |
| Route | `route2NN` | `tlsroute-hostname-mismatch-2` |
| Gateway | `exactGwNN` | `gateway-tls-exact-hostname` |
| Gateway | `wildcardGwNN` | `gateway-tls-wildcard-hostname` |

### Preconditions

- Namespaces must be ready
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteInvalidReferenceGrant

- **Description:** A single TLSRoute in the gateway-conformance-infra namespace, with a backendRef in another namespace without valid ReferenceGrant, should have the ResolvedRefs condition set to False
- **Manifests:** `tests/tlsroute-invalid-reference-grant.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `gateway-conformance-infra-test` |
| Gateway | `gwNN` | `gateway-tlsroute-referencegrant` |

### Preconditions

- Gateway and Routes must be accepted
- Resources must have expected conditions

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteListenerMixedTerminationNotSupported

- **Description:** When TLSRoute mixed termination/passthrough listener is NOT supported, a Gateway Listener with 2 distinct TLS modes on the same port MUST have Accepted=False with Reason=ProtocolConflict
- **Manifests:** `tests/tlsroute-listener-mixed-termination-not-supported.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`, `SupportTLSRouteModeTerminate`, `SupportTLSRouteModeMixed`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-tlsroute-mixed-termination-not-supported` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteListenerPassthroughSupportedKinds

- **Description:** A Gateway Listener with TLS mode Passthrough MUST include TLSRoute in SupportedKinds
- **Manifests:** `tests/tlsroute-listener-passthrough-supported-kinds.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-tlsroute-passthrough-supported-kind` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteListenerTerminateNotSupported

- **Description:** When TLSRoute termination is NOT supported, a Gateway Listener with TLS mode Terminate MUST have Accepted=False with Reason=UnsupportedValue
- **Manifests:** `tests/tlsroute-listener-terminate-not-supported.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`, `SupportTLSRouteModeTerminate`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-tlsroute-terminate-unsupported` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteListenerTerminateSupportedKinds

- **Description:** A Gateway Listener with TLS mode Terminate MUST include TLSRoute in SupportedKinds when termination is supported
- **Manifests:** `tests/tlsroute-listener-terminate-supported-kinds.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`, `SupportTLSRouteModeTerminate`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Gateway | `gwNN` | `gateway-tlsroute-terminate-supported` |

### Preconditions

- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---

## TLSRouteMixedTerminationSameNamespace

- **Description:** A Gateway with 2 TLS Listeners on different modes, on the same port must route the traffic correctly
- **Status:** Provisional
- **Manifests:** `tests/tlsroute-mixed-termination-same-namespace.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`, `SupportTLSRouteModeTerminate`, `SupportTLSRouteModeMixed`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeTerminateNN` | `gateway-conformance-mixed-terminateroute` |
| Route | `routePassthroughNN` | `gateway-conformance-mixed-passthroughroute` |
| Gateway | `gwNN` | `gateway-tlsroute-mixed-termination` |
| Resource | `caCertNN` | `tls-checks-ca-certificate` |
| Resource | `certNN` | `tls-passthrough-checks-certificate` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready
- Gateway must have expected listeners

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Makes TCP request and validates

---

## TLSRouteSimpleSameNamespace

- **Description:** A single TLSRoute in the gateway-conformance-infra namespace attaches to a Gateway in the same namespace
- **Manifests:** `tests/tlsroute-simple-same-namespace.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `gateway-conformance-infra-test` |
| Gateway | `gwNN` | `gateway-tlsroute` |
| Resource | `certNN` | `tls-passthrough-checks-certificate` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Makes TCP request and validates

---

## TLSRouteTerminateSimpleSameNamespace

- **Description:** A single TLSRoute in the gateway-conformance-infra namespace attaches to a Gateway using Terminate mode in the same namespace
- **Status:** Provisional
- **Manifests:** `tests/tlsroute-terminate-simple-same-namespace.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportTLSRoute`, `SupportTLSRouteModeTerminate`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `tlsroute-terminated-test` |
| Gateway | `gwNN` | `gateway-tlsroute-terminate` |
| Resource | `caCertNN` | `tls-checks-ca-certificate` |

### Preconditions

- Gateway and Routes must be accepted
- Namespaces must be ready

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

### Special Validations

- Makes TCP request and validates

---

## UDPRoute

- **Description:** Make sure UDPRoute is working
- **Status:** Provisional
- **Manifests:** `tests/udproute-simple.yaml`
- **Features:** `FeatureName`, `SupportUDPRoute`, `SupportGateway`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Kind | Variable | Name |
|------|----------|------|
| Route | `routeNN` | `udp-coredns` |
| Gateway | `gwNN` | `udp-gateway` |

### Preconditions

- Gateway and Routes must be accepted

### Test Behavior

*This test performs status/condition validation (no request-based test cases extracted from source).*

---
