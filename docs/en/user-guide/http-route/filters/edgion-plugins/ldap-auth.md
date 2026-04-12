# LDAP Auth (HTTPRoute Filter)

> **🔌 Edgion Extension**
>  
> LdapAuth is an LDAP authentication plugin provided by the `EdgionPlugins` CRD and is not part of the standard Gateway API.

LdapAuth can be used as an HTTPRoute filter or configured globally via `EdgionPlugins`. This page describes how to use it within HTTPRoute; for complete configuration parameters, see the [standalone plugin page](../../../edgion-plugins/ldap-auth.md).

## Using in HTTPRoute

### Minimal Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-ldap-auth-plugin
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-ldap-auth-plugin
  namespace: production
spec:
  requestPlugins:
    - type: LdapAuth
      config:
        ldapHost: ldap-service
        ldapPort: 389
        baseDn: "dc=example,dc=com"
        attribute: "uid"
```

## Related Documentation

- [LDAP Auth Full Documentation (all configuration parameters)](../../../edgion-plugins/ldap-auth.md)
- [Plugin Composition and References](../plugin-composition.md)
- [Filters Overview](../overview.md)
