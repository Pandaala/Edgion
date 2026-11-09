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
- Search mode: a service account finds the user's entry with an LDAP filter, so
  login names need not be part of the DN (Active Directory `sAMAccountName`,
  users spread across OUs)
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

A username containing a control character (including TAB and NUL) is rejected
with `401` before any LDAP request. In search mode, steps 3-4 are replaced by the
flow in [Search Mode](#search-mode).

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
| `bindDn` | String | none | Service-account **entry DN** (not a UPN or `DOMAIN\user`) in string form: attribute names such as `cn`, not OIDs, and no `#hex` values. Setting it selects search mode; mutually exclusive with `bindDnTemplate` |
| `bindPasswordRef` | Object | none | Secret holding the service-account password under the key `password`; required with `bindDn` |
| `searchFilter` | String | `(<attribute>={username})` | Search mode: RFC 4515 filter containing `{username}` (at most 8 times, at most 4096 bytes, nesting depth at most 16); an extensible match rule must be a numeric OID such as `1.2.840.113556.1.4.1941`, not a name |
| `searchBase` | String | `baseDn` | Search mode: base DN of the whole-subtree search; it (or `baseDn` when unset) must be a valid DN |
| `requiredGroupsAnyOf` | String[] | none | Search mode: group DNs; the user must be in at least one (1 to 32 entries, each a full DN) |
| `requiredGroupsAllOf` | String[] | none | Search mode: group DNs; the user must be in every one (1 to 32 entries). With `requiredGroupsAnyOf`, both must hold |
| `groupAttribute` | String | `memberOf` | Attribute on the user entry that lists group DNs; must be maintained by the directory and not writable by users |
| `nestedGroups` | Boolean | `false` | Match nested groups with the Active Directory in-chain rule `1.2.840.113556.1.4.1941` (not supported by OpenLDAP) |
| `headerType` | String | `ldap` | Authentication header scheme name |
| `hideCredentials` | Boolean | `false` | Whether to remove the authentication header before forwarding. If it cannot be removed, the request is denied with the normal `401` after `authFailureDelayMs` (access log `credential removal failed`, diagnostic `AuthHeaderMutationRejected`) instead of forwarding the credential |
| `authFailureDelayMs` | Integer | `0` | Delay before returning a credential-related 401 response, in milliseconds |
| `anonymous` | String | none | Anonymous username; applies only when both credential headers are missing |
| `realm` | String | `API Gateway` | `WWW-Authenticate` realm; [Character rules](../http-route/filters/edgion-plugins/README.md#authentication-realm) |
| `timeout` | Integer | `10000` | LDAP timeout (milliseconds), applied separately to connecting (DNS resolution, TCP, StartTLS, TLS handshake) and to the bind plus unbind (in search mode: service bind, search, user bind, with group requirements the service re-bind and group search, and unbind together); one LDAP exchange can take up to twice this value, plus `authFailureDelayMs` on a 401 or 403 |
| `keepalive` | Integer | `60000` | Reserved field |
| `credentialIdentifierHeader` | String | `X-Credential-Identifier` | Header for passing the authenticated username (in search mode: the entry's `attribute` value) |
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

## Search Mode

Set `bindDn` to make the plugin look the user up instead of building a DN. For
every request, on one connection, the plugin:

1. binds as the service account (`bindDn` + the `password` key of `bindPasswordRef`);
2. searches `searchBase` (whole subtree, at most 2 entries) with `searchFilter`,
   requesting only `attribute`;
3. requires exactly one entry, then binds as that entry with the client's password;
4. forwards the entry's single value of `attribute` (for example `uid` or
   `sAMAccountName`) in `credentialIdentifierHeader`, not the string the client typed.

```yaml
config:
  ldapHost: "ad.corp.example.com"
  ldapPort: 636
  ldaps: true
  baseDn: "DC=corp,DC=example,DC=com"
  attribute: "sAMAccountName"
  bindDn: "CN=svc-gateway,OU=Service Accounts,DC=corp,DC=example,DC=com"
  bindPasswordRef:
    name: ldap-service-account     # Secret with key "password"
  searchFilter: "(&(objectClass=user)(sAMAccountName={username}))"
  tls:
    validation:
      caCertificateRefs:
        - kind: Secret
          name: corp-ca
```

Rules and guarantees:

- Search mode requires `ldaps` or `startTls` with certificate verification; a local
  `tls.verify: false` without a SAN policy is rejected. An inherited global
  `verify=false` only logs a warning at plugin build time.
- The username is inserted into the compiled filter as raw bytes; characters such as
  `*`, `(`, `)` or `\` in a username cannot change the filter (no LDAP injection).
- No entry, more than one entry, an entry without exactly one valid `attribute`
  value (non-empty, no surrounding spaces or control characters), an entry DN the
  plugin cannot parse, and the service account's own entry all return the same
  `401` as a wrong password. `attribute` must be the name the directory returns (aliases such as
  `userid` for `uid` are not resolved); a multi-valued attribute denies that user.
- A rejected or unresolved service-account password returns the generic `401` and
  increments `edgion_ldap_auth_errors_total{kind="service_bind_fail"}`; alert on it.
  A `bindPasswordRef` Secret without a usable `password` value also shows
  `ResolvedRefs=False` (`MissingKeys` or `DecodeError`) on the resource status.
- Timing: an unknown user skips the final bind, so it answers one LDAP round trip
  faster than a wrong password. `authFailureDelayMs` slows guessing but does not
  hide this difference; bound probing with `RateLimitLocal`.
- The service account needs permission to search `searchBase` and read `attribute`.

### Group Requirements

With `requiredGroupsAnyOf` and/or `requiredGroupsAllOf`, a user whose password is correct must
also be a member of the listed groups:

```yaml
config:
  # ... search mode settings as above ...
  requiredGroupsAnyOf:
    - "CN=API Users,OU=Groups,DC=corp,DC=example,DC=com"
    - "CN=API Admins,OU=Groups,DC=corp,DC=example,DC=com"
  requiredGroupsAllOf:
    - "CN=MFA Enrolled,OU=Groups,DC=corp,DC=example,DC=com"
  nestedGroups: true   # Active Directory only
```

- Only after the user's bind succeeds, the plugin binds as the service account again and checks
  the user's own entry with a base-scope search on `(<groupAttribute>=<group DN>)` items
  (`requiredGroupsAllOf`: all of them; `requiredGroupsAnyOf`: at least one). A wrong password
  never triggers a group check; it returns the usual `401`.
- A member continues as usual; a non-member gets `403 Forbidden` (after `authFailureDelayMs`).
  Note that 401 versus 403 tells a client whether the password of a non-member was correct. If
  that matters, put the group condition into `searchFilter` instead; non-members then get the
  same `401` as a wrong password.
- Groups are full DNs, compared by the directory. An empty list (`[]`), a `null` value or a `null`
  entry (for example an empty templated value) in the stored resource is rejected with a resource
  status error. Caution: when `kubectl apply`, Helm or Argo CD update an existing resource, a field
  set to `null` in the manifest is sent as a merge-patch delete, so the key disappears and the
  requirement is removed without an error. Remove group requirements only deliberately and check the
  applied resource. A misspelled key (for example `requiredGroupAnyOf`) is not rejected by the
  plugin either: check the resource status for unknown-field warnings after applying a configuration.
- **Prerequisite: the directory maintains `groupAttribute` and users cannot write it.** Active
  Directory's `memberOf` is a system-maintained back-link. OpenLDAP needs the `memberof` overlay
  (with `refint`), and its `memberOf` is user-modifiable unless an ACL protects it — place a rule
  such as `access to attrs=memberOf by * read` before any `by self write` rule. A `memberOf`
  value written directly counts as membership. Never point `groupAttribute` at an attribute users
  can edit (for example `description`).
- The service account needs read access to `groupAttribute` on user entries; without it, every
  user gets `403`.
- Active Directory: the primary group (usually `Domain Users`) is not listed in `memberOf`.
  `nestedGroups: true` uses the in-chain rule, which can be slow on large group graphs; OpenLDAP
  does not implement it, so members get `403` there.
- Cost: one more bind and one more search per successful login.

## Error Semantics

| Scenario | Status Code | Description |
|----------|------------|-------------|
| Missing credentials (and `anonymous` not enabled) | `401` | Returns `WWW-Authenticate` |
| Invalid credential format | `401` | Generic authentication failure |
| Wrong LDAP credentials | `401` | Generic authentication failure |
| LDAP service unreachable/timeout | `503` | Service unavailable |
| StartTLS refused by the server (non-zero result code) | `503` | Counted as `starttls_rejected`, separate from `unreachable` |
| LDAP destination denied by egress policy | `503` | Same generic body as unreachable |
| Username contains a control character | `401` | Rejected before any LDAP request |
| Search mode: username longer than 1024 bytes | `401` | Rejected before any LDAP request |
| Group requirements: correct password, not a member | `403` | `403 Forbidden`; checked only after a successful user bind |
| Search mode: unknown, ambiguous or unsuitable user entry | `401` | Same as a wrong password |
| Search mode: service-account password rejected or unresolved | `401` | Counted as `service_bind_fail` |
| Search mode: search failed (referral, no such base, protocol error, a directory error for this username; with group requirements also a failed group search or a re-bind failure other than a rejected password) | `503` | Counted as `search_fail` |

## Security Recommendations

- Prefer `ldaps: true` or `startTls: true` in production
- Keep the effective `tls.verify: true`; `false` is operator responsibility and should only be used for testing
- Enable `hideCredentials: true` to prevent credentials from being passed to upstream
- Enable account lockout and brute-force protection policies on the LDAP server
- Stack the `RateLimitLocal` plugin and size the LDAP service for the protected request rate

> **Accepted tradeoff — per-request LDAP connection.** Each authentication opens a new TCP (and optional TLS) connection, runs `simple_bind` (in search mode: service bind, search, user bind, plus a service re-bind and a group search with group requirements), and unbinds immediately. There is no connection pool because LDAP `simple_bind` changes the connection's authenticated identity; unsafe cross-user reuse is worse than the connection overhead.

## Related Documentation

- [BasicAuth Plugin](../http-route/filters/edgion-plugins/basic-auth.md) - Basic username/password authentication
- [ForwardAuth Plugin](forward-auth.md) - External authentication plugin
- [JwtAuth Plugin](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT token authentication
