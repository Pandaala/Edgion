# LDAP Auth Plugin

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
- Supports authentication result caching (`cacheTtl`)
- Supports LDAPS / StartTLS (via `ldaps` / `startTls`)

## Workflow

1. Read request header: first reads `Proxy-Authorization`, then `Authorization`
2. Parse format: `{headerType} base64(username:password)`
3. Optional cache hit: if cached, allow through directly
4. Build Bind DN:
   - Default: `{attribute}={username},{baseDn}`
   - Template: `bindDnTemplate` replaces `{username}`
5. LDAP Simple Bind verification
6. On success, inject upstream headers and continue forwarding

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `ldapHost` | String | None | LDAP host, required |
| `ldapPort` | Integer | `389` | LDAP port |
| `ldaps` | Boolean | `false` | Whether to enable LDAPS |
| `startTls` | Boolean | `false` | Whether to enable StartTLS (mutually exclusive with `ldaps`) |
| `verifyLdapHost` | Boolean | `true` | Whether to verify the certificate hostname |
| `baseDn` | String | None | Base DN, required |
| `attribute` | String | None | User attribute, required (e.g., `uid`/`cn`) |
| `bindDnTemplate` | String | None | Custom Bind DN template, must contain `{username}` |
| `headerType` | String | `ldap` | Authentication header scheme name |
| `hideCredentials` | Boolean | `false` | Whether to remove the authentication header before forwarding |
| `anonymous` | String | None | Anonymous username (when configured, allows unauthenticated requests) |
| `realm` | String | `API Gateway` | `WWW-Authenticate` realm |
| `cacheTtl` | Integer | `60` | Authentication cache TTL (seconds), `0` disables caching |
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
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        ldapPort: 389
        startTls: true
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        timeout: 3000
```

## Using `headerType: basic` Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth-basic-scheme
  namespace: default
spec:
  requestPlugins:
    - enable: true
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
  namespace: default
spec:
  requestPlugins:
    - enable: true
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
- Keep `verifyLdapHost: true`
- Enable `hideCredentials: true` to prevent credentials from being passed to upstream
- Set a reasonable `cacheTtl` (e.g., 60–300 seconds) to balance performance and credential revocation latency
- Enable account lockout and brute-force protection policies on the LDAP server
