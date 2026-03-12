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
| RequestMirror | Request mirroring | Coming soon |

### Edgion Extension Filters

> **🔌 Edgion Extension**
> 
> The following plugins are implemented via the `EdgionPlugins` CRD and are Edgion extension features.

Referenced through `ExtensionRef` to EdgionPlugins resources:

| Plugin | Description | Documentation |
|--------|-------------|---------------|
| BasicAuth | HTTP basic authentication | [Details](./edgion-plugins/basic-auth.md) |
| LdapAuth | LDAP directory authentication | [Details](../../edgion-plugins/ldap-auth.md) |
| CORS | Cross-origin resource sharing | [Details](./edgion-plugins/cors.md) |
| CSRF | CSRF protection | [Details](./edgion-plugins/csrf.md) |
| IpRestriction | IP allowlist/denylist | [Details](./edgion-plugins/ip-restriction.md) |
| RateLimit | Rate limiting | [Details](./edgion-plugins/rate-limit.md) |

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
