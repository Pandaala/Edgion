# HTTPRoute Conformance Test Source Detail

> Gateway API v1.5.0 conformance tests extracted from Go source.
> Every test case with exact request/response expectations for manual verification.

**Total tests: 52** | **Total HTTP test cases: 172**

## Table of Contents

### Core Routing

1. [HTTPRouteCrossNamespace](#httproutecrossnamespace) (1 cases)
2. [HTTPRouteMatching](#httproutematching) (9 cases)
3. [HTTPRouteMatchingAcrossRoutes](#httproutematchingacrossroutes) (8 cases)
4. [HTTPRouteReferenceGrant](#httproutereferencegrant) (2 cases)
5. [HTTPRouteSimpleSameNamespace](#httproutesimplesamenamespace) (1 cases)

### Path Matching

6. [HTTPRouteExactPathMatching](#httprouteexactpathmatching) (6 cases)
7. [HTTPRoutePathMatchOrder](#httproutepathmatchorder) (6 cases)

### Header Matching

8. [HTTPRouteHeaderMatching](#httprouteheadermatching) (11 cases)

### Method Matching

9. [HTTPRouteMethodMatching](#httproutemethodmatching) (5 cases)

### Query Param Matching

10. [HTTPRouteQueryParamMatching](#httproutequeryparammatching) (5 cases)

### Hostname / Listener

11. [HTTPRouteHTTPSListener](#httproutehttpslistener) (1 cases)
12. [HTTPRouteHTTPSListenerDetectMisdirectedRequests](#httproutehttpslistenerdetectmisdirectedrequests) (1 cases)
13. [HTTPRouteHostnameIntersection](#httproutehostnameintersection) (2 cases)
14. [HTTPRouteInvalidParentRefNotMatchingListenerPort](#httprouteinvalidparentrefnotmatchinglistenerport) (validation only)
15. [HTTPRouteListenerHostnameMatching](#httproutelistenerhostnamematching) (8 cases)
16. [HTTPRouteListenerPortMatching](#httproutelistenerportmatching) (5 cases)

### Redirect Filters

17. [HTTPRoute303Redirect](#httproute303redirect) *(Provisional)* (1 cases)
18. [HTTPRoute307Redirect](#httproute307redirect) *(Provisional)* (1 cases)
19. [HTTPRoute308Redirect](#httproute308redirect) *(Provisional)* (1 cases)
20. [HTTPRouteRedirectHostAndStatus](#httprouteredirecthostandstatus) (2 cases)
21. [HTTPRouteRedirectPath](#httprouteredirectpath) (6 cases)
22. [HTTPRouteRedirectPort](#httprouteredirectport) (4 cases)
23. [HTTPRouteRedirectPortAndScheme](#httprouteredirectportandscheme) (6 cases)
24. [HTTPRouteRedirectScheme](#httprouteredirectscheme) (4 cases)

### Request Header Modifier

25. [HTTPRouteBackendRequestHeaderModifier](#httproutebackendrequestheadermodifier) (delegates to HTTPRouteRequestHeaderModifier)
26. [HTTPRouteRequestHeaderModifier](#httprouterequestheadermodifier) (7 cases)
27. [HTTPRouteRequestHeaderModifierBackendWeights](#httprouterequestheadermodifierbackendweights) (1 cases)

### Response Header Modifier

28. [HTTPRouteResponseHeaderModifier](#httprouteresponseheadermodifier) (11 cases)

### Rewrite Filters

29. [HTTPRouteRewriteHost](#httprouterewritehost) (3 cases)
30. [HTTPRouteRewritePath](#httprouterewritepath) (6 cases)

### Mirror Filters

31. [HTTPRouteRequestMirror](#httprouterequestmirror) (2 cases)
32. [HTTPRouteRequestMultipleMirrors](#httprouterequestmultiplemirrors) (2 cases)
33. [HTTPRouteRequestPercentageMirror](#httprouterequestpercentagemirror) *(Provisional)* (3 cases)

### Backend / Weight / Protocol

34. [HTTPRouteBackendProtocolH2C](#httproutebackendprotocolh2c) (1 cases)
35. [HTTPRouteBackendProtocolWebSocket](#httproutebackendprotocolwebsocket) (validation only)
36. [HTTPRouteServiceTypes](#httprouteservicetypes) (1 cases)
37. [HTTPRouteWeight](#httprouteweight) (5 cases)

### Timeout

38. [HTTPRouteTimeoutBackendRequest](#httproutetimeoutbackendrequest) (3 cases)
39. [HTTPRouteTimeoutRequest](#httproutetimeoutrequest) (3 cases)

### CORS

40. [HTTPRouteCORS](#httproutecors) (17 cases)
41. [HTTPRouteCORSAllowCredentialsBehavior](#httproutecorsallowcredentialsbehavior) (3 cases)

### Invalid / Error Cases

42. [HTTPRouteDisallowedKind](#httproutedisallowedkind) (validation only)
43. [HTTPRouteInvalidBackendRefUnknownKind](#httprouteinvalidbackendrefunknownkind) (1 cases)
44. [HTTPRouteInvalidCrossNamespaceBackendRef](#httprouteinvalidcrossnamespacebackendref) (1 cases)
45. [HTTPRouteInvalidCrossNamespaceParentRef](#httprouteinvalidcrossnamespaceparentref) (validation only)
46. [HTTPRouteInvalidNonExistentBackendRef](#httprouteinvalidnonexistentbackendref) (1 cases)
47. [HTTPRouteInvalidParentRefNotMatchingSectionName](#httprouteinvalidparentrefnotmatchingsectionname) (validation only)
48. [HTTPRouteInvalidParentRefSectionNameNotMatchingPort](#httprouteinvalidparentrefsectionnamenotmatchingport) (validation only)
49. [HTTPRouteInvalidReferenceGrant](#httprouteinvalidreferencegrant) (1 cases)
50. [HTTPRoutePartiallyInvalidViaInvalidReferenceGrant](#httproutepartiallyinvalidviainvalidreferencegrant) (2 cases)

### Status / Generation

51. [HTTPRouteNamedRule](#httproutenamedrule) *(Provisional)* (2 cases)
52. [HTTPRouteObservedGenerationBump](#httprouteobservedgenerationbump) (validation only)

---

## HTTPRoute303Redirect

- **File:** `httproute-303-redirect.go`
- **Description:** An HTTPRoute with a 303 redirect filter
- **Status:** Provisional
- **Manifests:** `tests/httproute-303-redirect.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRoute303RedirectStatusCode`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `303-redirect` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `POST /see-other`

**Request:**

| Field | Value |
|-------|-------|
| Method | `POST` |
| Path | `/see-other` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `303` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Path | `/see-other` |

---

## HTTPRoute307Redirect

- **File:** `httproute-307-redirect.go`
- **Description:** An HTTPRoute with a 307 path redirect filter
- **Status:** Provisional
- **Manifests:** `tests/httproute-307-redirect.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRoute307RedirectStatusCode`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `307-redirect` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `GET /temporary`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/temporary` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `307` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Path | `/temporary` |

---

## HTTPRoute308Redirect

- **File:** `httproute-308-redirect.go`
- **Description:** An HTTPRoute with a 308 path redirect filter
- **Status:** Provisional
- **Manifests:** `tests/httproute-308-redirect.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRoute308RedirectStatusCode`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `308-redirect` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `GET /permanent`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/permanent` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `308` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Path | `/permanent` |

---

## HTTPRouteBackendProtocolH2C

- **File:** `httproute-backend-protocol-h2c.go`
- **Description:** A HTTPRoute with a BackendRef that has an appProtocol kubernetes.io/h2c should be functional
- **Manifests:** `tests/httproute-backend-protocol-h2c.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteBackendProtocolH2C`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `backend-protocol-h2c` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

### Special Validations

- Tests HTTP/2 cleartext (h2c) backend protocol

---

## HTTPRouteBackendProtocolWebSocket

- **File:** `httproute-backend-protocol-websocket.go`
- **Description:** A HTTPRoute with a BackendRef that has an appProtocol kubernetes.io/ws should be functional
- **Manifests:** `tests/httproute-backend-protocol-websocket.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteBackendProtocolWebSocket`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `backend-protocol-ws` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Behavior

*This test performs status/condition validation only (no HTTP request test cases).*

### Special Validations

- Tests WebSocket upgrade support

---

## HTTPRouteBackendRequestHeaderModifier

- **File:** `httproute-request-header-modifier.go`
- **Description:** An HTTPRoute backend has request header modifier filters applied correctly
- **Manifests:** `tests/httproute-request-header-modifier-backend.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteBackendRequestHeaderModification`
- **Namespace:** `gateway-conformance-infra`

### Test Behavior

This test delegates its test function to `HTTPRouteRequestHeaderModifier.Test`, running the same test cases with a different manifest.

### Special Validations

- Delegates test execution to HTTPRouteRequestHeaderModifier.Test

---

## HTTPRouteCORS

- **File:** `httproute-cors.go`
- **Description:** An HTTPRoute with CORS filter should allow CORS requests from specified origins
- **Manifests:** `tests/httproute-cors.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteCORS`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `cors-multiple-origins-methods-headers` | `gateway-conformance-infra` |
| HTTPRoute | `cors-wildcard-methods` | `gateway-conformance-infra` |
| HTTPRoute | `cors-wildcard-origin` | `gateway-conformance-infra` |
| HTTPRoute | `cors-wildcard-methods-headers` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (17)

#### Case 1: `OPTIONS /cors-1`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-1` |
| Header `Origin` | `https://www.foo.com` |
| Header `access-control-request-method` | `GET` |
| Header `access-control-request-headers` | `x-header-1, x-header-2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 2: `OPTIONS /cors-1`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-1` |
| Header `Origin` | `https://www.bar.com` |
| Header `access-control-request-method` | `GET` |
| Header `access-control-request-headers` | `x-header-1, x-header-2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 3: `OPTIONS /cors-1`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-1` |
| Header `Origin` | `https://xpto.www.bar.com` |
| Header `access-control-request-method` | `GET` |
| Header `access-control-request-headers` | `x-header-1, x-header-2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 4: `OPTIONS /cors-1`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-1` |
| Header `Origin` | `https://foobar.com` |
| Header `access-control-request-method` | `GET` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 5: `GET /cors-1`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/cors-1` |
| Header `Origin` | `https://www.foo.com` |
| Header `access-control-request-method` | `GET` |
| Header `access-control-request-headers` | `x-header-1, x-header-2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 6: `GET /cors-1`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/cors-1` |
| Header `Origin` | `https://www.bar.com` |
| Header `access-control-request-method` | `GET` |
| Header `access-control-request-headers` | `x-header-1, x-header-2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 7: `GET /cors-1`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/cors-1` |
| Header `Origin` | `https://foobar.com` |
| Header `access-control-request-method` | `GET` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |
| Absent Response Headers | `access-control-allow-origin` |

#### Case 8: `OPTIONS /cors-2`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-2` |
| Header `Origin` | `https://www.foo.com` |
| Header `access-control-request-method` | `POST` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 9: `OPTIONS /cors-2`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-2` |
| Header `Origin` | `https://www.foo.com` |
| Header `access-control-request-method` | `POST` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Absent Response Headers | `Access-Control-Allow-Credentials` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 10: `OPTIONS /cors-wildcard-origin`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-wildcard-origin` |
| Header `Origin` | `https://foobar.com` |
| Header `access-control-request-method` | `PUT` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 11: `OPTIONS /cors-wildcard-origin`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-wildcard-origin` |
| Header `Origin` | `https://foobar.com:12345` |
| Header `access-control-request-method` | `PUT` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 12: `PUT /cors-wildcard-origin`

**Request:**

| Field | Value |
|-------|-------|
| Method | `PUT` |
| Path | `/cors-wildcard-origin` |
| Header `Origin` | `https://foobar.com:12345` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 13: `PUT /cors-wildcard-origin`

**Request:**

| Field | Value |
|-------|-------|
| Method | `PUT` |
| Path | `/cors-wildcard-origin` |
| Header `Origin` | `https://foobar.com` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 14: `OPTIONS /cors-wildcard-methods-headers`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-wildcard-methods-headers` |
| Header `Origin` | `https://other.foo.com` |
| Header `access-control-request-method` | `PUT` |
| Header `access-control-request-headers` | `x-header-1, x-header-2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 15: `OPTIONS /cors-wildcard-methods-headers-unauth`

**Request:**

| Field | Value |
|-------|-------|
| Method | `OPTIONS` |
| Path | `/cors-wildcard-methods-headers-unauth` |
| Header `Origin` | `https://other.foo.com` |
| Header `access-control-request-method` | `PUT` |
| Header `access-control-request-headers` | `x-header-1, x-header-2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `` |

#### Case 16: `GET /cors-wildcard-methods-headers`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/cors-wildcard-methods-headers` |
| Header `Origin` | `https://other.foo.com` |
| Header `Authorization` | `Bearer test` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 17: `GET /cors-wildcard-methods-headers-unauth`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/cors-wildcard-methods-headers-unauth` |
| Header `Origin` | `https://other.foo.com` |
| Header `Authorization` | `Bearer test` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteCORSAllowCredentialsBehavior

- **File:** `httproute-cors-allow-credentials-behavior.go`
- **Description:** An HTTPRoute with CORS includes Access-Control-Allow-Credentials only when configured as 'true', and then only alongside a matching Access-Control-Allow-Origin.
- **Manifests:** `tests/httproute-cors-allow-credentials-behavior.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteCORS`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `cors-allow-credentials` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (3)

#### Case 1: `GET /cors-behavior-creds-false`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/cors-behavior-creds-false` |
| Header `Cookie` | `sid=abc123` |
| Header `Authorization` | `Bearer test` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |
| Absent Response Headers | `Access-Control-Allow-Credentials` |

#### Case 2: `GET /cors-behavior-creds-true`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/cors-behavior-creds-true` |
| Header `Origin` | `http://not-app.example` |
| Header `Cookie` | `sid=abc123` |
| Header `Authorization` | `Bearer test` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |
| Absent Response Headers | `Access-Control-Allow-Credentials` |

#### Case 3: `GET /cors-behavior-creds-true`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/cors-behavior-creds-true` |
| Header `Cookie` | `sid=abc123` |
| Header `Authorization` | `Bearer test` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |
| Response Header `Access-Control-Allow-Credentials` | `true` |

---

## HTTPRouteCrossNamespace

- **File:** `httproute-cross-namespace.go`
- **Description:** A single HTTPRoute in the gateway-conformance-web-backend namespace should attach to Gateway in another namespace
- **Manifests:** `tests/httproute-cross-namespace.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `backend-namespaces` | `gateway-conformance-infra` |
| HTTPRoute | `cross-namespace` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `web-backend` |
| Namespace | `gateway-conformance-web-backend` |

---

## HTTPRouteDisallowedKind

- **File:** `httproute-disallowed-kind.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace should fail to attach to a Gateway with no listeners that allow the HTTPRoute kind
- **Manifests:** `tests/httproute-disallowed-kind.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportTLSRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `tlsroutes-only` | `gateway-conformance-infra` |
| HTTPRoute | `disallowed-kind` | `gateway-conformance-infra` |

### Preconditions

- HTTPRoute must have ResolvedRefs=True

### Test Behavior

*This test performs status/condition validation only (no HTTP request test cases).*

---

## HTTPRouteExactPathMatching

- **File:** `httproute-exact-path-matching.go`
- **Description:** A single HTTPRoute with exact path matching for different backends
- **Manifests:** `tests/httproute-exact-path-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `exact-matching` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (6)

#### Case 1: `GET /one`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /two`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/two` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 4: `GET /one/example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/one/example` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 5: `GET /two/`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/two/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 6: `GET /Two`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/Two` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

---

## HTTPRouteHTTPSListener

- **File:** `httproute-https-listener.go`
- **Description:** HTTPRoute attaches to a Gateway's HTTPS listener in the same namespace
- **Manifests:** `tests/httproute-https-listener.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace-with-https-listener` | `gateway-conformance-infra` |
| HTTPRoute | `httproute-https-test` | `gateway-conformance-infra` |
| HTTPRoute | `httproute-https-test-no-hostname` | `gateway-conformance-infra` |
| HTTPRoute | `tls-validity-checks-certificate` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteHTTPSListenerDetectMisdirectedRequests

- **File:** `httproute-https-listener-detect-misdirected-requests.go`
- **Description:** HTTPS listeners on the same port detect misdirected requests and return HTTP 421 when appropriate
- **Manifests:** `tests/httproute-https-listener-detect-misdirected-requests.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportGatewayHTTPSListenerDetectMisdirectedRequests`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace-with-https-listener` | `gateway-conformance-infra` |
| HTTPRoute | `tls-validity-checks-certificate` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `GET /detect-misdirected-requests`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/detect-misdirected-requests` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

### Special Validations

- Validates HTTP 421 Misdirected Request

---

## HTTPRouteHeaderMatching

- **File:** `httproute-header-matching.go`
- **Description:** A single HTTPRoute with header matching for different backends
- **Manifests:** `tests/httproute-header-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `header-matching` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (11)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Version` | `one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Version` | `two` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Version` | `two` |
| Header `Color` | `orange` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Version` | `two` |
| Header `Color` | `blue` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 5: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Color` | `orange` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 6: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Some-Other-Header` | `one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 7: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Color` | `blue` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 8: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Color` | `green` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 9: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Color` | `red` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 10: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Color` | `yellow` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 11: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Color` | `purple` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

---

## HTTPRouteHostnameIntersection

- **File:** `httproute-hostname-intersection.go`
- **Description:** HTTPRoutes should attach to listeners only if they have intersecting hostnames, and should accept requests only for the intersecting hostnames
- **Manifests:** `tests/httproute-hostname-intersection.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `httproute-hostname-intersection` | `gateway-conformance-infra` |
| Gateway | `httproute-hostname-intersection-all` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True
- HTTPRoute must have specific parent refs

### Test Cases (2)

#### Case 1: `GET /s5`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/s5` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 2: `GET /s5`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/s5` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

---

## HTTPRouteInvalidBackendRefUnknownKind

- **File:** `httproute-invalid-backendref-unknown-kind.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace should set a ResolvedRefs status False with reason InvalidKind when attempting to bind to a Gateway in the same namespace if the route has a BackendRef that points to an unknown Kind.
- **Manifests:** `tests/httproute-invalid-backendref-unknown-kind.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `invalid-backend-ref-unknown-kind` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Cases (1)

#### Case 1: `GET /v2`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `500` |

---

## HTTPRouteInvalidCrossNamespaceBackendRef

- **File:** `httproute-invalid-cross-namespace-backend-ref.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace should set a ResolvedRefs status False with reason RefNotPermitted when attempting to bind to a Gateway in the same namespace if the route has a BackendRef Service in the gateway-conformance-web-backend namespace and a ReferenceGrant granting permission to route to that Service does not exist
- **Manifests:** `tests/httproute-invalid-cross-namespace-backend-ref.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `invalid-cross-namespace-backend-ref` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `500` |

---

## HTTPRouteInvalidCrossNamespaceParentRef

- **File:** `httproute-invalid-cross-namespace-parent-ref.go`
- **Description:** A single HTTPRoute in the gateway-conformance-web-backend namespace should fail to attach to a Gateway in another namespace that it is not allowed to
- **Manifests:** `tests/httproute-invalid-cross-namespace-parent-ref.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `invalid-cross-namespace-parent-ref` | `gateway-conformance-infra` |

### Preconditions

- HTTPRoute must have ResolvedRefs=True

### Test Behavior

*This test performs status/condition validation only (no HTTP request test cases).*

---

## HTTPRouteInvalidNonExistentBackendRef

- **File:** `httproute-invalid-nonexistent-backendref.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace should set a ResolvedRefs status False with reason BackendNotFound and return 500 when binding to a Gateway in the same namespace if the route has a BackendRef Service that does not exist
- **Manifests:** `tests/httproute-invalid-nonexistent-backendref.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `invalid-nonexistent-backend-ref` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `500` |

---

## HTTPRouteInvalidParentRefNotMatchingListenerPort

- **File:** `httproute-invalid-parentref-not-matching-listener-port.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace should set the Accepted status to False with reason NoMatchingParent when attempting to bind to a Gateway that does not have a matching ListenerPort.
- **Manifests:** `tests/httproute-invalid-parentref-not-matching-listener-port.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteDestinationPortMatching`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `httproute-listener-not-matching-route-port` | `gateway-conformance-infra` |

### Preconditions

- HTTPRoute must have ResolvedRefs=True

### Test Behavior

*This test performs status/condition validation only (no HTTP request test cases).*

---

## HTTPRouteInvalidParentRefNotMatchingSectionName

- **File:** `httproute-invalid-parentref-not-matching-section-name.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace should set the Accepted status to False with reason NoMatchingParent when attempting to bind to a Gateway that does not have a matching SectionName.
- **Manifests:** `tests/httproute-invalid-parentref-not-matching-section-name.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `httproute-listener-not-matching-section-name` | `gateway-conformance-infra` |

### Test Behavior

*This test performs status/condition validation only (no HTTP request test cases).*

---

## HTTPRouteInvalidParentRefSectionNameNotMatchingPort

- **File:** `httproute-invalid-parentref-section-name-not-matching-port.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace should set the Accepted status to False with reason NoMatchingParent when attempting to bind to a Gateway that SectionName does not match Port value.
- **Manifests:** `tests/httproute-invalid-parentref-section-name-not-matching-port.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteParentRefPort`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `gateway-with-one-not-matching-port-and-section-name-route` | `gateway-conformance-infra` |
| HTTPRoute | `httproute-listener-section-name-not-matching-port` | `gateway-conformance-infra` |

### Test Behavior

*This test performs status/condition validation only (no HTTP request test cases).*

---

## HTTPRouteInvalidReferenceGrant

- **File:** `httproute-invalid-reference-grant.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace, with a backendRef in another namespace without valid ReferenceGrant, should have the ResolvedRefs condition set to False and not forward HTTP requests to any backend
- **Manifests:** `tests/httproute-invalid-reference-grant.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `reference-grant` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `500` |
| Backend | `web-backend` |
| Namespace | `gateway-conformance-web-backend` |

---

## HTTPRouteListenerHostnameMatching

- **File:** `httproute-listener-hostname-matching.go`
- **Description:** Multiple HTTP listeners with the same port and different hostnames, each with a different HTTPRoute
- **Manifests:** `tests/httproute-listener-hostname-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `httproute-listener-hostname-matching` | `gateway-conformance-infra` |
| HTTPRoute | `backend-v1` | `gateway-conformance-infra` |
| HTTPRoute | `backend-v2` | `gateway-conformance-infra` |
| HTTPRoute | `backend-v3` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (8)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v3` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v3` |
| Namespace | `gateway-conformance-infra` |

#### Case 5: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v3` |
| Namespace | `gateway-conformance-infra` |

#### Case 6: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v3` |
| Namespace | `gateway-conformance-infra` |

#### Case 7: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 8: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

---

## HTTPRouteListenerPortMatching

- **File:** `httproute-listener-port-matching.go`
- **Description:** Multiple HTTP listeners with different ports, each with a different HTTPRoute
- **Manifests:** `tests/httproute-listener-port-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteParentRefPort`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `httproute-listener-port-matching` | `gateway-conformance-infra` |
| HTTPRoute | `backend-v1` | `gateway-conformance-infra` |
| HTTPRoute | `backend-v2` | `gateway-conformance-infra` |
| HTTPRoute | `backend-v3` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (5)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v3` |
| Namespace | `gateway-conformance-infra` |

#### Case 5: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

---

## HTTPRouteMatching

- **File:** `httproute-matching.go`
- **Description:** A single HTTPRoute with path and header matching for different backends
- **Manifests:** `tests/httproute-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `matching` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (9)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/example` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Version` | `one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `GET /v2`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 5: `GET /v2/example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2/example` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 6: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Version` | `two` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 7: `GET /v2/`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 8: `GET /v2example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2example` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 9: `GET /foo/v2/example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/foo/v2/example` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteMatchingAcrossRoutes

- **File:** `httproute-matching-across-routes.go`
- **Description:** Two HTTPRoutes with path matching for different backends
- **Manifests:** `tests/httproute-matching-across-routes.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `matching-part1` | `gateway-conformance-infra` |
| HTTPRoute | `matching-part2` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (8)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/example` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/example` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `GET /example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/example` |
| Header `Version` | `one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 5: `GET /v2`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 6: `GET /v2`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 7: `GET /v2/example`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2/example` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 8: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |
| Header `Version` | `two` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteMethodMatching

- **File:** `httproute-method-matching.go`
- **Description:** A single HTTPRoute with method matching for different backends
- **Manifests:** `tests/httproute-method-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteMethodMatching`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `method-matching` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (5)

#### Case 1: `POST /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `POST` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /path1`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/path1` |
| Header `version` | `one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `PATCH /path3`

**Request:**

| Field | Value |
|-------|-------|
| Method | `PATCH` |
| Path | `/path3` |
| Header `version` | `three` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `PUT /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `PUT` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 5: `PATCH /path5`

**Request:**

| Field | Value |
|-------|-------|
| Method | `PATCH` |
| Path | `/path5` |
| Header `version` | `four` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteNamedRule

- **File:** `httproute-named-rule.go`
- **Description:** An HTTPRoute with a named HTTPRouteRule
- **Status:** Provisional
- **Manifests:** `tests/httproute-named-rule.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteNamedRouteRule`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `http-named-rules` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (2)

#### Case 1: `GET /named`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/named` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /unnamed`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/unnamed` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteObservedGenerationBump

- **File:** `httproute-observed-generation-bump.go`
- **Description:** A HTTPRoute in the gateway-conformance-infra namespace should update the observedGeneration in all of it's Status.Conditions after an update to the spec
- **Manifests:** `tests/httproute-observed-generation-bump.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `observed-generation-bump` | `gateway-conformance-infra` |

### Preconditions

- HTTPRoute must have ResolvedRefs=True

### Test Behavior

*This test performs status/condition validation only (no HTTP request test cases).*

### Special Validations

- Validates ObservedGeneration bump

---

## HTTPRoutePartiallyInvalidViaInvalidReferenceGrant

- **File:** `httproute-partially-invalid-via-invalid-reference-grant.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace should attach to a Gateway in the same namespace if the route has a backendRef Service in the gateway-conformance-app-backend namespace and a ReferenceGrant exists but does not grant permission to route to that specific Service
- **Manifests:** `tests/httproute-partially-invalid-via-invalid-reference-grant.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `invalid-reference-grant` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Cases (2)

#### Case 1: `GET /v2`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/v2` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `500` |

#### Case 2: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `app-backend-v1` |
| Namespace | `gateway-conformance-app-backend` |

---

## HTTPRoutePathMatchOrder

- **File:** `httproute-path-match-order.go`
- **Description:** An HTTPRoute where there are multiple matches routing to any given backend follows match order precedence
- **Manifests:** `tests/httproute-path-match-order.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (6)

#### Case 1: `GET /match/exact/one`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/match/exact/one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v3` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /match/exact`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/match/exact` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /match`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/match` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `GET /match/prefix/one/any`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/match/prefix/one/any` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 5: `GET /match/prefix/any`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/match/prefix/any` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 6: `GET /match/any`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/match/any` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v3` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteQueryParamMatching

- **File:** `httproute-query-param-matching.go`
- **Description:** A single HTTPRoute with query param matching for different backends
- **Manifests:** `tests/httproute-query-param-matching.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteQueryParamMatching`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (5)

#### Case 1: `GET /?animal=whale`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/?animal=whale` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /path1?animal=whale`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/path1?animal=whale` |
| Header `version` | `one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /path3?animal=shark`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/path3?animal=shark` |
| Header `version` | `three` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `GET /?animal=shark`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/?animal=shark` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `404` |

#### Case 5: `GET /path5?animal=hydra`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/path5?animal=hydra` |
| Header `version` | `four` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteRedirectHostAndStatus

- **File:** `httproute-redirect-host-and-status.go`
- **Description:** An HTTPRoute with hostname and statusCode redirect filters
- **Manifests:** `tests/httproute-redirect-host-and-status.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `redirect-host-and-status` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (2)

#### Case 1: `GET /hostname-redirect`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/hostname-redirect` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |

#### Case 2: `GET /host-and-status`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/host-and-status` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `301` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |

---

## HTTPRouteRedirectPath

- **File:** `httproute-redirect-path.go`
- **Description:** An HTTPRoute with scheme redirect filter
- **Manifests:** `tests/httproute-redirect-path.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRoutePathRedirect`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `redirect-path` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (6)

#### Case 1: `GET /original-prefix/lemon`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/original-prefix/lemon` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Path | `/replacement-prefix/lemon` |

#### Case 2: `GET /full/path/original`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/full/path/original` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Path | `/full-path-replacement` |

#### Case 3: `GET /path-and-host`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/path-and-host` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Path | `/replacement-prefix` |

#### Case 4: `GET /path-and-status`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/path-and-status` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `301` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Path | `/replacement-prefix` |

#### Case 5: `GET /full-path-and-host`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/full-path-and-host` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Path | `/replacement-full` |

#### Case 6: `GET /full-path-and-status`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/full-path-and-status` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `301` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Path | `/replacement-full` |

---

## HTTPRouteRedirectPort

- **File:** `httproute-redirect-port.go`
- **Description:** An HTTPRoute with a port redirect filter
- **Manifests:** `tests/httproute-redirect-port.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRoutePortRedirect`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `redirect-port` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (4)

#### Case 1: `GET /port`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/port` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Port | `8083` |

#### Case 2: `GET /port-and-host`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/port-and-host` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Port | `8083` |

#### Case 3: `GET /port-and-status`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/port-and-status` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `301` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Port | `8083` |

#### Case 4: `GET /port-and-host-and-status`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/port-and-host-and-status` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Port | `8083` |

---

## HTTPRouteRedirectPortAndScheme

- **File:** `httproute-redirect-port-and-scheme.go`
- **Description:** An HTTPRoute with port and scheme redirect filter
- **Manifests:** `tests/httproute-redirect-port-and-scheme.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRoutePortRedirect`, `SupportGatewayPort8080`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `http-route-for-listener-on-port-80` | `gateway-conformance-infra` |
| HTTPRoute | `tls-validity-checks-certificate` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (6)

#### Case 1: `GET /scheme-nil-and-port-nil`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-nil-and-port-nil` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Scheme | `http` |

#### Case 2: `GET /scheme-nil-and-port-80`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-nil-and-port-80` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Scheme | `http` |

#### Case 3: `GET /scheme-nil-and-port-8080`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-nil-and-port-8080` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Scheme | `http` |
| Port | `8080` |

#### Case 4: `GET /scheme-https-and-port-nil`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-https-and-port-nil` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Scheme | `https` |

#### Case 5: `GET /scheme-https-and-port-443`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-https-and-port-443` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Scheme | `https` |

#### Case 6: `GET /scheme-https-and-port-8443`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-https-and-port-8443` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Scheme | `https` |
| Port | `8443` |

---

## HTTPRouteRedirectScheme

- **File:** `httproute-redirect-scheme.go`
- **Description:** An HTTPRoute with a scheme redirect filter
- **Manifests:** `tests/httproute-redirect-scheme.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteSchemeRedirect`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `redirect-scheme` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (4)

#### Case 1: `GET /scheme`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Scheme | `https` |

#### Case 2: `GET /scheme-and-host`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-and-host` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Scheme | `https` |

#### Case 3: `GET /scheme-and-status`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-and-status` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `301` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Scheme | `https` |

#### Case 4: `GET /scheme-and-host-and-status`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/scheme-and-host-and-status` |
| Follow Redirect | No |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `302` |
| Namespace | `gateway-conformance-infra` |

**Expected Redirect:**

| Field | Value |
|-------|-------|
| Host | `example.org` |
| Scheme | `https` |

---

## HTTPRouteReferenceGrant

- **File:** `httproute-reference-grant.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace, with a backendRef in the gateway-conformance-web-backend namespace, should attach to Gateway in the gateway-conformance-infra namespace
- **Manifests:** `tests/httproute-reference-grant.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportReferenceGrant`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `reference-grant` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (2)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `web-backend` |
| Namespace | `gateway-conformance-web-backend` |

#### Case 2: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `500` |

---

## HTTPRouteRequestHeaderModifier

- **File:** `httproute-request-header-modifier.go`
- **Description:** An HTTPRoute has request header modifier filters applied correctly
- **Manifests:** `tests/httproute-request-header-modifier.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `request-header-modifier` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (7)

#### Case 1: `GET /set`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/set` |
| Header `Some-Other-Header` | `val` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `Some-Other-Header` | `val` |
| Header `X-Header-Set` | `set-overwrites-values` |

#### Case 2: `GET /set`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/set` |
| Header `Some-Other-Header` | `val` |
| Header `X-Header-Set` | `some-other-value` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `Some-Other-Header` | `val` |
| Header `X-Header-Set` | `set-overwrites-values` |

#### Case 3: `GET /add`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/add` |
| Header `Some-Other-Header` | `val` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `Some-Other-Header` | `val` |
| Header `X-Header-Add` | `add-appends-values` |

#### Case 4: `GET /add`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/add` |
| Header `Some-Other-Header` | `val` |
| Header `X-Header-Add` | `some-other-value` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `Some-Other-Header` | `val` |
| Header `X-Header-Add` | `some-other-value,add-appends-values` |

#### Case 5: `GET /remove`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/remove` |
| Header `X-Header-Remove` | `val` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Absent Headers | `X-Header-Remove` |

#### Case 6: `GET /multiple`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/multiple` |
| Header `X-Header-Set-2` | `set-val-2` |
| Header `X-Header-Add-2` | `add-val-2` |
| Header `X-Header-Remove-2` | `remove-val-2` |
| Header `Another-Header` | `another-header-val` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `X-Header-Set-1` | `header-set-1` |
| Header `X-Header-Set-2` | `header-set-2` |
| Header `X-Header-Add-1` | `header-add-1` |
| Header `X-Header-Add-2` | `add-val-2,header-add-2` |
| Header `X-Header-Add-3` | `header-add-3` |
| Header `Another-Header` | `another-header-val` |
| Absent Headers | `X-Header-Remove-1`, `X-Header-Remove-2` |

#### Case 7: `GET /case-insensitivity`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/case-insensitivity` |
| Header `x-header-set` | `original-val-set` |
| Header `x-header-add` | `original-val-add` |
| Header `x-header-remove` | `original-val-remove` |
| Header `Another-Header` | `another-header-val` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `X-Header-Set` | `header-set` |
| Header `X-Header-Add` | `original-val-add,header-add` |
| Header `Another-Header` | `another-header-val` |
| Absent Headers | `x-header-remove`, `X-Header-Remove` |

---

## HTTPRouteRequestHeaderModifierBackendWeights

- **File:** `httproute-request-header-modifier-backend-weights.go`
- **Description:** An HTTPRoute with backend request header modifier filter sends traffic to the correct backends
- **Manifests:** `tests/httproute-request-header-modifier-backend-weights.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRouteBackendRequestHeaderModification`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `request-header-modifier-backend-weights` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteRequestMirror

- **File:** `httproute-request-mirror.go`
- **Description:** An HTTPRoute with request mirror filter
- **Manifests:** `tests/httproute-request-mirror.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteRequestMirror`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `request-mirror` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Cases (2)

#### Case 1: `GET /mirror`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/mirror` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Mirrored To:** `infra-backend-v2`

#### Case 2: `GET /mirror-and-modify-headers`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/mirror-and-modify-headers` |
| Header `X-Header-Remove` | `remove-val` |
| Header `X-Header-Add-Append` | `append-val-1` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `X-Header-Add` | `header-val-1` |
| Header `X-Header-Add-Append` | `append-val-1,header-val-2` |
| Header `X-Header-Set` | `set-overwrites-values` |
| Absent Headers | `X-Header-Remove` |

**Mirrored To:** `infra-backend-v2`

---

## HTTPRouteRequestMultipleMirrors

- **File:** `httproute-request-multiple-mirrors.go`
- **Description:** An HTTPRoute with multiple request mirror filters
- **Manifests:** `tests/httproute-request-multiple-mirrors.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteRequestMirror`, `SupportHTTPRouteRequestMultipleMirrors`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `request-multiple-mirrors` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted

### Test Cases (2)

#### Case 1: `GET /multi-mirror`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/multi-mirror` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Mirrored To:** `infra-backend-v2`, `infra-backend-v3`

#### Case 2: `GET /multi-mirror-and-modify-request-headers`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/multi-mirror-and-modify-request-headers` |
| Header `X-Header-Remove` | `remove-val` |
| Header `X-Header-Add-Append` | `append-val-1` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `X-Header-Add` | `header-val-1` |
| Header `X-Header-Add-Append` | `append-val-1,header-val-2` |
| Header `X-Header-Set` | `set-overwrites-values` |
| Absent Headers | `X-Header-Remove` |

**Mirrored To:** `infra-backend-v2`, `infra-backend-v3`

---

## HTTPRouteRequestPercentageMirror

- **File:** `httproute-request-percentage-mirror.go`
- **Description:** An HTTPRoute with percentage based request mirroring
- **Status:** Provisional
- **Manifests:** `tests/httproute-request-percentage-mirror.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteRequestPercentageMirror`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (3)

#### Case 1: `GET /percent-mirror`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/percent-mirror` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Mirrored To:** `infra-backend-v2`

#### Case 2: `GET /percent-mirror-fraction`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/percent-mirror-fraction` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Mirrored To:** `infra-backend-v2`

#### Case 3: `GET /percent-mirror-and-modify-headers`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/percent-mirror-and-modify-headers` |
| Header `X-Header-Remove` | `remove-val` |
| Header `X-Header-Add-Append` | `append-val-1` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `X-Header-Add` | `header-val-1` |
| Header `X-Header-Add-Append` | `append-val-1,header-val-2` |
| Header `X-Header-Set` | `set-overwrites-values` |
| Absent Headers | `X-Header-Remove` |

**Mirrored To:** `infra-backend-v2`

---

## HTTPRouteResponseHeaderModifier

- **File:** `httproute-response-header-modifier.go`
- **Description:** An HTTPRoute has response header modifier filters applied correctly
- **Manifests:** `tests/httproute-response-header-modifier.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteResponseHeaderModification`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `response-header-modifier` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (11)

#### Case 1: `GET /set`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/set` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |
| Response Header `Some-Other-Header` | `val` |
| Response Header `X-Header-Set` | `set-overwrites-values` |

#### Case 2: `GET /set`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/set` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |
| Response Header `Some-Other-Header` | `val` |
| Response Header `X-Header-Set` | `set-overwrites-values` |

#### Case 3: `GET /add`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/add` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |
| Response Header `Some-Other-Header` | `val` |
| Response Header `X-Header-Add` | `add-appends-values` |

#### Case 4: `GET /add`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/add` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |
| Response Header `Some-Other-Header` | `val` |
| Response Header `X-Header-Add` | `some-other-value,add-appends-values` |

#### Case 5: `GET /remove`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/remove` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |
| Absent Response Headers | `X-Header-Remove` |

#### Case 6: `GET /multiple`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/multiple` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |
| Response Header `X-Header-Set-1` | `header-set-1` |
| Response Header `X-Header-Set-2` | `header-set-2` |
| Response Header `X-Header-Add-1` | `header-add-1` |
| Response Header `X-Header-Add-2` | `add-val-2,header-add-2` |
| Response Header `X-Header-Add-3` | `header-add-3` |
| Response Header `Another-Header` | `another-header-val` |

#### Case 7: `GET /case-insensitivity`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/case-insensitivity` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Response Header `X-Header-Set` | `header-set` |
| Response Header `X-Header-Add` | `original-val-add,header-add` |
| Response Header `X-Lowercase-Add` | `lowercase-add` |
| Response Header `X-Mixedcase-Add-1` | `mixedcase-add-1` |
| Response Header `X-Mixedcase-Add-2` | `mixedcase-add-2` |
| Response Header `X-Uppercase-Add` | `uppercase-add` |
| Response Header `Another-Header` | `another-header-val` |

#### Case 8: `GET `

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

#### Case 9: `GET `

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |

#### Case 10: `GET `

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 11: `GET /response-and-request-header-modifiers`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/response-and-request-header-modifiers` |
| Header `X-Header-Remove` | `remove-val` |
| Header `X-Header-Add-Append` | `append-val-1` |
| Header `X-Header-Echo` | `echo` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |
| Response Header `X-Header-Set-1` | `header-set-1` |
| Response Header `X-Header-Set-2` | `header-set-2` |
| Response Header `X-Header-Add-1` | `header-add-1` |
| Response Header `X-Header-Add-2` | `add-val-2,header-add-2` |
| Response Header `Another-Header` | `another-header-val` |
| Response Header `X-Header-Echo` | `echo` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `X-Header-Add` | `header-val-1` |
| Header `X-Header-Set` | `set-overwrites-values` |
| Header `X-Header-Add-Append` | `append-val-1,header-val-2` |
| Header `X-Header-Echo` | `echo` |
| Absent Headers | `X-Header-Remove` |

---

## HTTPRouteRewriteHost

- **File:** `httproute-rewrite-host.go`
- **Description:** An HTTPRoute with hostname rewrite filter
- **Manifests:** `tests/httproute-rewrite-host.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteHostRewrite`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `rewrite-host` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (3)

#### Case 1: `GET /one`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/one` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /two`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/two` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /rewrite-host-and-modify-headers`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/rewrite-host-and-modify-headers` |
| Header `X-Header-Remove` | `remove-val` |
| Header `X-Header-Add-Append` | `append-val-1` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Header `X-Header-Add` | `header-val-1` |
| Header `X-Header-Add-Append` | `append-val-1,header-val-2` |
| Header `X-Header-Set` | `set-overwrites-values` |
| Absent Headers | `X-Header-Remove` |

---

## HTTPRouteRewritePath

- **File:** `httproute-rewrite-path.go`
- **Description:** An HTTPRoute with path rewrite filter
- **Manifests:** `tests/httproute-rewrite-path.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRoutePathRewrite`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `rewrite-path` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (6)

#### Case 1: `GET /prefix/one/two`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/prefix/one/two` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `/one/two` |

#### Case 2: `GET /strip-prefix/three`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/strip-prefix/three` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `/three` |

#### Case 3: `GET /strip-prefix`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/strip-prefix` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `/` |

#### Case 4: `GET /full/one/two`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/full/one/two` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `/one` |

#### Case 5: `GET /full/rewrite-path-and-modify-headers/test`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/full/rewrite-path-and-modify-headers/test` |
| Header `X-Header-Remove` | `remove-val` |
| Header `X-Header-Add-Append` | `append-val-1` |
| Header `X-Header-Set` | `set-val` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `/test` |
| Header `X-Header-Add` | `header-val-1` |
| Header `X-Header-Add-Append` | `append-val-1,header-val-2` |
| Header `X-Header-Set` | `set-overwrites-values` |
| Absent Headers | `X-Header-Remove` |

#### Case 6: `GET /prefix/rewrite-path-and-modify-headers/one`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/prefix/rewrite-path-and-modify-headers/one` |
| Header `X-Header-Remove` | `remove-val` |
| Header `X-Header-Add-Append` | `append-val-1` |
| Header `X-Header-Set` | `set-val` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

**Expected Request at Backend:**

| Field | Value |
|-------|-------|
| Path | `/prefix/one` |
| Header `X-Header-Add` | `header-val-1` |
| Header `X-Header-Add-Append` | `append-val-1,header-val-2` |
| Header `X-Header-Set` | `set-overwrites-values` |
| Absent Headers | `X-Header-Remove` |

---

## HTTPRouteServiceTypes

- **File:** `httproute-service-types.go`
- **Description:** A single HTTPRoute should be able to route traffic to various service type backends
- **Manifests:** `tests/httproute-service-types.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteSimpleSameNamespace

- **File:** `httproute-simple-same-namespace.go`
- **Description:** A single HTTPRoute in the gateway-conformance-infra namespace attaches to a Gateway in the same namespace
- **Manifests:** `tests/httproute-simple-same-namespace.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `gateway-conformance-infra-test` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (1)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteTimeoutBackendRequest

- **File:** `httproute-timeout-backend-request.go`
- **Description:** An HTTPRoute with backend request timeout
- **Manifests:** `tests/httproute-timeout-backend-request.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteBackendTimeout`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `backend-request-timeout` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (3)

#### Case 1: `GET /backend-timeout`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/backend-timeout` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /backend-timeout?delay=1s`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/backend-timeout?delay=1s` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `504` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /disable-backend-timeout?delay=1s`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/disable-backend-timeout?delay=1s` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteTimeoutRequest

- **File:** `httproute-timeout-request.go`
- **Description:** An HTTPRoute with request timeout
- **Manifests:** `tests/httproute-timeout-request.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`, `SupportHTTPRouteRequestTimeout`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| Gateway | `same-namespace` | `gateway-conformance-infra` |
| HTTPRoute | `request-timeout` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (3)

#### Case 1: `GET /request-timeout`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/request-timeout` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /request-timeout?delay=1s`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/request-timeout?delay=1s` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `504` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /disable-request-timeout?delay=1s`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/disable-request-timeout?delay=1s` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

---

## HTTPRouteWeight

- **File:** `httproute-weight.go`
- **Description:** An HTTPRoute with weighted backends
- **Manifests:** `tests/httproute-weight.yaml`
- **Features:** `FeatureName`, `SupportGateway`, `SupportHTTPRoute`
- **Namespace:** `gateway-conformance-infra`

### Resources

| Resource | Name | Namespace |
|----------|------|-----------|
| HTTPRoute | `tls-validity-checks-certificate` | `gateway-conformance-infra` |
| HTTPRoute | `tls-validity-checks-client-certificate` | `gateway-conformance-infra` |
| HTTPRoute | `tls-validity-checks-per-port-client-certificate` | `gateway-conformance-infra` |

### Preconditions

- Gateway and HTTPRoutes must be accepted
- HTTPRoute must have ResolvedRefs=True

### Test Cases (5)

#### Case 1: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 2: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v1` |
| Namespace | `gateway-conformance-infra` |

#### Case 3: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

#### Case 4: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Backend | `infra-backend-v2` |
| Namespace | `gateway-conformance-infra` |

#### Case 5: `GET /`

**Request:**

| Field | Value |
|-------|-------|
| Method | `GET` |
| Path | `/` |

**Expected Response:**

| Field | Value |
|-------|-------|
| Status Code | `200` |
| Namespace | `gateway-conformance-infra` |

### Special Validations

- Validates ObservedGeneration bump
- Validates weighted traffic distribution
- Tests WebSocket upgrade support
- Tests HTTP/2 cleartext (h2c) backend protocol
- Validates HTTP 421 Misdirected Request

---
