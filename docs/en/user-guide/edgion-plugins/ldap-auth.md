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
- Supports missing-credential-only anonymous pass-through (`anonymous`)
- Supports hiding credential headers (`hideCredentials`)
- Supports LDAPS / StartTLS (via `ldaps` / `startTls`)
- Applies the gateway outbound egress policy to each LDAP connection at dial time

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
| `tls.verify` | Boolean | inherited | Verify the LDAP server certificate. An unset local value inherits `EdgionGatewayConfig.spec.outboundTls.verify`; the final default is `true`. |
| `tls.validation.caCertificateRefs` | Array | inherited | CA bundle Secret references. A present local `validation` block replaces the global validation block as a whole. |
| `tls.validation.subjectAltNames` | Array | inherited | Additional SAN policy. A non-empty policy forces full certificate verification even when `verify` was requested as `false`. |
| `tls.clientCertificateRef` | Object | inherited | Client mTLS identity Secret. Local client identity is resolved independently from the validation block; otherwise the global identity is inherited. |
| `baseDn` | String | **Required** | Base DN |
| `attribute` | String | **Required** | User attribute (e.g., `uid` / `cn`) |
| `bindDnTemplate` | String | none | Custom Bind DN template, must contain `{username}` |
| `headerType` | String | `ldap` | Authentication header scheme name |
| `hideCredentials` | Boolean | `false` | Whether to remove the authentication header before forwarding |
| `authFailureDelayMs` | Integer | `0` | Delay before returning a credential-related 401 response, in milliseconds |
| `anonymous` | String | none | Anonymous username; applies only when both credential headers are missing |
| `realm` | String | `API Gateway` | `WWW-Authenticate` realm; [Character rules](../http-route/filters/edgion-plugins/README.md#authentication-realm) |
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
    - type: LdapAuth
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

### TLS, global inheritance, and warnings

`tls` is used by the Gateway connection only when `ldaps: true` or
`startTls: true`. For TLS-enabled LDAP, an unset local `tls.verify` inherits the
global `EdgionGatewayConfig.spec.outboundTls.verify`. A local `tls.validation`
block replaces the global validation block as a whole; its fields are not
merged. Client identity is inherited independently: a local
`tls.clientCertificateRef` wins, otherwise a configured global identity is
used.

```yaml
requestPlugins:
  - type: LdapAuth
    config:
      ldapHost: ldap.example.com
      ldapPort: 636
      ldaps: true
      baseDn: "dc=example,dc=com"
      attribute: uid
      tls:
        verify: true
        validation:
          caCertificateRefs:
            - kind: Secret
              name: ldap-ca
        clientCertificateRef:
          name: ldap-client-identity
```

The TLS warnings have distinct meanings:

- When effective skip-verification is actually used, the shared outbound-TLS
  resolver warns that verification is disabled.
- A non-empty SAN policy overrides a requested `verify: false`, warns about the
  override, enforces chain and SAN verification, and retains an available mTLS
  identity.
- When a local `clientCertificateRef` is configured while effective
  skip-verification is used, the client identity is intentionally omitted and
  LdapAuth emits an additional M4 warning that the client certificate is not
  sent.

For plain LDAP (`ldaps: false` and `startTls: false`), Gateway plugin
construction does not call the outbound-TLS resolver, build a TLS client
configuration, or emit these shared/M4 warnings. The Controller still processes
any declared TLS Secret references, registers their dependencies, checks
Secrets and ReferenceGrants, and may update status, log, or requeue the plugin.
Omit unused TLS configuration for plain LDAP; do not rely on it being inert in
the control plane.

## Outbound egress

Each LDAP connection resolves `ldapHost` at connect time and dials only
addresses allowed by the gateway outbound egress policy. Loopback, link-local,
and unspecified destinations are always rejected. RFC1918 private destinations
follow `outbound_egress_block_private` / `EDGION_OUTBOUND_BLOCK_PRIVATE`
(default `false`), so typical in-cluster LDAP remains reachable unless that
switch is enabled. Denied destinations fail closed with the same generic 503 as
an unreachable directory. LDAPS and StartTLS still verify the original
`ldapHost` hostname. See
[Egress policy (SSRF protection)](../../ops-guide/infrastructure/connectors.md#egress-policy-ssrf-protection).

## Using `headerType: basic` Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth-basic-scheme
  namespace: production
spec:
  requestPlugins:
    - type: LdapAuth
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
    - type: LdapAuth
      config:
        ldapHost: ldap.example.com
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        anonymous: "guest-user"
        hideCredentials: true
```

Anonymous access is used only when neither credential header is present.
Malformed headers and credentials rejected by LDAP still return 401. When
allowing anonymous access, the following headers are injected:
- `X-Credential-Identifier: guest-user`
- `X-Anonymous-Consumer: true`

## Error Semantics

| Scenario | Status Code | Description |
|----------|------------|-------------|
| Missing credentials (and `anonymous` not enabled) | `401` | Returns `WWW-Authenticate` |
| Invalid credential format | `401` | Generic authentication failure |
| Wrong LDAP credentials | `401` | Generic authentication failure |
| LDAP service unreachable/timeout | `503` | Service unavailable |
| LDAP destination denied by egress policy | `503` | Same generic body as unreachable |

## Security Recommendations

- Prefer `ldaps: true` or `startTls: true` in production
- Keep the effective `tls.verify: true`; `false` is operator responsibility and should only be used for testing
- Enable `hideCredentials: true` to prevent credentials from being passed to upstream
- Enable account lockout and brute-force protection policies on the LDAP server
- Stack the `RateLimit` plugin and size the LDAP service for the protected request rate

> **Accepted tradeoff — per-request LDAP connection.** Each authentication opens a new TCP (and optional TLS) connection, runs `simple_bind`, and unbinds immediately. There is no connection pool because LDAP `simple_bind` changes the connection's authenticated identity; unsafe cross-user reuse is worse than the connection overhead.

## Related Documentation

- [BasicAuth Plugin](../http-route/filters/edgion-plugins/basic-auth.md) - Basic username/password authentication
- [ForwardAuth Plugin](forward-auth.md) - External authentication plugin
- [JwtAuth Plugin](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT token authentication
