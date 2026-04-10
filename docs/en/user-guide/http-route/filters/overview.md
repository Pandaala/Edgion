# Filters Overview

Filters are used to modify requests or responses during the routing process.

## Filter Types

### Gateway API Standard Filters

| Type | Description | Documentation |
|------|-------------|---------------|
| RequestHeaderModifier | Modify request headers | [Details](./gateway-api/request-header-modifier.md) |
| ResponseHeaderModifier | Modify response headers | [Details](./gateway-api/response-header-modifier.md) |
| RequestRedirect | Request redirect | [Details](./gateway-api/request-redirect.md) |
| URLRewrite | URL rewrite | [Details](./gateway-api/url-rewrite.md) |
| RequestMirror | Request mirroring | [Details](./gateway-api/request-mirror.md) |

### Edgion Extension Filters

> **🔌 Edgion Extension**
> 
> The following plugins are implemented via the `EdgionPlugins` CRD and are Edgion extension features.

Referenced through `ExtensionRef` to EdgionPlugins resources:

| Plugin | Description | Documentation |
|--------|-------------|---------------|
| AllEndpointStatus | Query all backend endpoint statuses | [Details](./edgion-plugins/all-endpoint-status.md) |
| BandwidthLimit | Bandwidth throttling | [Details](./edgion-plugins/bandwidth-limit.md) |
| BasicAuth | HTTP basic authentication | [Details](./edgion-plugins/basic-auth.md) |
| CORS | Cross-origin resource sharing | [Details](./edgion-plugins/cors.md) |
| CSRF | CSRF protection | [Details](./edgion-plugins/csrf.md) |
| CtxSet | Context variable extraction | [Details](./edgion-plugins/ctx-setter.md) |
| DirectEndpoint | Direct connection to a specific endpoint | [Details](./edgion-plugins/direct-endpoint.md) |
| DSL | DSL scripting engine | [Details](./edgion-plugins/dsl.md) |
| DynamicUpstream | Dynamic upstream selection | [Details](./edgion-plugins/dynamic-upstream.md) |
| ForwardAuth | External authentication delegation | [Details](./edgion-plugins/forward-auth.md) |
| HeaderCertAuth | Header/certificate authentication | [Details](./edgion-plugins/header-cert-auth.md) |
| HmacAuth | HMAC signature verification | [Details](./edgion-plugins/hmac-auth.md) |
| IpRestriction | IP allowlist/denylist | [Details](./edgion-plugins/ip-restriction.md) |
| JweDecrypt | JWE Token decryption | [Details](./edgion-plugins/jwe-decrypt.md) |
| JwtAuth | JWT authentication | [Details](./edgion-plugins/jwt-auth.md) |
| KeyAuth | API Key authentication | [Details](./edgion-plugins/key-auth.md) |
| LdapAuth | LDAP directory authentication | [Details](./edgion-plugins/ldap-auth.md) |
| Mock | Mock response | [Details](./edgion-plugins/mock.md) |
| OpenidConnect | OpenID Connect authentication | [Details](./edgion-plugins/openid-connect.md) |
| ProxyRewrite | Proxy rewrite | [Details](./edgion-plugins/proxy-rewrite.md) |
| RateLimit | Rate limiting | [Details](./edgion-plugins/rate-limit.md) |
| RealIp | Client real IP extraction | [Details](./edgion-plugins/real-ip.md) |
| RegionRoute | Region-based routing | [Details](./edgion-plugins/region-route.md) |
| RequestMirror | Request mirroring | [Details](./edgion-plugins/request-mirror.md) |
| RequestRestriction | Request filtering | [Details](./edgion-plugins/request-restriction.md) |
| ResponseRewrite | Response rewrite | [Details](./edgion-plugins/response-rewrite.md) |

## Filter Execution Order

```
Request → RequestHeaderModifier → ExtensionRef(plugins) → URLRewrite → Backend
Backend → ResponseHeaderModifier → Response
```

## Configuration Examples

### Using Standard Filters

```yaml
filters:
  - type: RequestHeaderModifier
    requestHeaderModifier:
      add:
        - name: X-Gateway
          value: edgion
```

### Using Edgion Plugins

```yaml
filters:
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: EdgionPlugins
      name: my-cors-plugin
```

## Related Documentation

- [HTTPRoute Overview](../overview.md)
- [Backend Configuration](../backends/README.md)
