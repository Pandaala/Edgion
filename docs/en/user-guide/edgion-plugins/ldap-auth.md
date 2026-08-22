# LDAP Auth Plugin

> **🔌 Edgion extension**
> 
> LdapAuth is an LDAP directory authentication plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

The LDAP Auth plugin delegates gateway authentication to an enterprise LDAP/AD directory service. The plugin parses `username:password` from request headers, then validates credentials using LDAP Simple Bind.

Use cases:
- Unified account system (LDAP / Active Directory)
- No need to maintain local user passwords in the gateway
- Alignment with enterprise account lifecycle management

## Features

- Supports `Authorization` / `Proxy-Authorization` authentication headers
- `Proxy-Authorization` has higher priority than `Authorization`
- Supports custom authentication scheme names (`headerType`, e.g., `ldap` / `basic`)
- Supports anonymous degradation (`anonymous`)
- Supports hiding credential headers (`hideCredentials`)
- Supports LDAPS / StartTLS (via `ldaps` / `startTls`)

## Workflow

1. Read request header: first reads `Proxy-Authorization`, then `Authorization`
2. Parse format: `{headerType} base64(username:password)`
3. Build Bind DN:
   - Default: `{attribute}={username},{baseDn}`
   - Template: `bindDnTemplate` replaces `{username}`
   - `{username}` is treated as a DN attribute value and escaped per RFC 4514
     (special characters like `, + " \ < > ; =` and leading `#` / surrounding spaces
     are escaped to their `\HH` hex form). The template must therefore describe a
     DN; using it as a search-filter template is not supported.
4. LDAP Simple Bind verification
5. On success, inject upstream headers and continue forwarding

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `ldapHost` | String | **Required** | LDAP host |
| `ldapPort` | Integer | `389` | LDAP port |
| `ldaps` | Boolean | `false` | Whether to enable LDAPS |
| `startTls` | Boolean | `false` | Whether to enable StartTLS (mutually exclusive with `ldaps`) |
| `verify` | Boolean | `true` | Verify the LDAP server's TLS certificate. When `false`, peer certificate validation is skipped (operator responsibility; a WARN is logged at startup). |
| `baseDn` | String | **Required** | Base DN |
| `attribute` | String | **Required** | User attribute (e.g., `uid` / `cn`) |
| `bindDnTemplate` | String | none | Custom Bind DN template, must contain `{username}` |
| `headerType` | String | `ldap` | Authentication header scheme name |
| `hideCredentials` | Boolean | `false` | Whether to remove the authentication header before forwarding |
| `anonymous` | String | none | Anonymous username (when configured, allows unauthenticated requests) |
| `realm` | String | `API Gateway` | `WWW-Authenticate` realm |
| `timeout` | Integer | `10000` | LDAP timeout (milliseconds) |
| `keepalive` | Integer | `60000` | Reserved field |
| `credentialIdentifierHeader` | String | `X-Credential-Identifier` | Header for passing the authenticated username |
| `anonymousHeader` | String | `X-Anonymous-Consumer` | Anonymous indicator header |

## Minimal Configuration Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth
  namespace: production
spec:
  requestPlugins:
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        ldapPort: 389
        startTls: true
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        timeout: 3000
```

LDAP authentication results are not cached. Every credential-bearing request
is checked against the authoritative LDAP service.

## Using `headerType: basic` Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth-basic-scheme
  namespace: production
spec:
  requestPlugins:
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        headerType: "basic"
```

Clients can use Basic authentication directly:

```bash
curl -u alice:password123 https://api.example.com/protected
```

## Anonymous Degradation Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth-anon
  namespace: production
spec:
  requestPlugins:
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        anonymous: "guest-user"
        hideCredentials: true
```

When allowing anonymous access, the following headers are injected:
- `X-Credential-Identifier: guest-user`
- `X-Anonymous-Consumer: true`

## Error Semantics

| Scenario | Status Code | Description |
|----------|------------|-------------|
| Missing credentials (and `anonymous` not enabled) | `401` | Returns `WWW-Authenticate` |
| Invalid credential format | `401` | Generic authentication failure |
| Wrong LDAP credentials | `401` | Generic authentication failure |
| LDAP service unreachable/timeout | `503` | Service unavailable |

## Security Recommendations

- Prefer `ldaps: true` or `startTls: true` in production
- Keep `verify: true` (the default); `verify: false` is operator responsibility and should only be used for testing
- Enable `hideCredentials: true` to prevent credentials from being passed to upstream
- Enable account lockout and brute-force protection policies on the LDAP server
- Stack the `RateLimit` plugin and size the LDAP service for the protected request rate

> **Accepted tradeoff — per-request LDAP connection.** Each authentication opens a new TCP (and optional TLS) connection, runs `simple_bind`, and unbinds immediately. There is no connection pool because LDAP `simple_bind` changes the connection's authenticated identity; unsafe cross-user reuse is worse than the connection overhead.

## Related Documentation

- [BasicAuth Plugin](../http-route/filters/edgion-plugins/basic-auth.md) - Basic username/password authentication
- [ForwardAuth Plugin](forward-auth.md) - External authentication plugin
- [JwtAuth Plugin](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT token authentication
