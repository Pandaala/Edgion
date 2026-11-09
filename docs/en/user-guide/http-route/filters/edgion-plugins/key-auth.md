# Key Auth Plugin

> **🔌 Edgion extension**
> 
> KeyAuth is an API Key authentication plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

Key Auth validates API Keys carried in requests from various sources including Headers, Query parameters, and Cookies to control access.

**How it works**:
1. Extracts the API Key from configured sources (Header / Query / Cookie)
2. Compares against valid keys stored in Kubernetes Secrets
3. On success: allows access, optionally forwards key metadata to upstream
4. On failure: returns 401 status code

## Quick Start

### Create API Key Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: api-keys
  namespace: production
type: Opaque
stringData:
  keys.yaml: |
    - key: "my-secret-api-key-1"
      username: "user1"
    - key: "my-secret-api-key-2"
      username: "user2"
```

### Configure the Plugin

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: key-auth-plugin
  namespace: production
spec:
  requestPlugins:
      type: KeyAuth
      config:
        keySources:
          - type: header
            name: "X-API-Key"
        secretGroups:
          - name: default
            secretRefs:
              - name: api-keys
```

---

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `keySources` | Array\<Object\> | `[{type:"header",name:"apikey"}, {type:"query",name:"apikey"}]` | Key source list (see sub-table) |
| `hideCredentials` | Boolean | `false` | Remove API Key from request after validation |
| `authFailureDelayMs` | Integer | `0` | Delay response on auth failure (brute force protection) |
| `anonymous` | String | none | Anonymous username; unauthenticated requests pass through |
| `realm` | String | `"API Gateway"` | Authentication realm name; [Character rules](README.md#authentication-realm) |
| `keyField` | String | `"key"` | Field name storing the key value in Secret |
| `secretGroups` | Array\<Object\> | none | Named groups of Kubernetes Secret references (see sub-table) |
| `upstreamHeaderFields` | Array\<String\> | `[]` | Extra headers to forward upstream (from key metadata) |

### KeySource Sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `type` | String | **Required** | Source type: `header` / `query` / `cookie` / `ctx` |
| `name` | String | **Required** | Source name (header name, query param name, etc.) |

### KeyAuthSecretGroup Sub-fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Group name. Charset: `^[a-zA-Z0-9]([-_.a-zA-Z0-9]*[a-zA-Z0-9])?$`, max 63 chars. Surfaces in PluginLog as `group=<name>` |
| `description` | String | none | Human-readable description (not used by runtime) |
| `secretRefs` | Array\<Object\> | **Required** | Secret references for this group; must be non-empty |

---

## Usage Scenarios

### Scenario 1: Key from Header

```yaml
requestPlugins:
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      secretGroups:
        - name: default
          secretRefs:
            - name: api-keys
      hideCredentials: true
```

**Test**:
```bash
curl -H "X-API-Key: my-secret-api-key-1" https://api.example.com/resource
```

### Scenario 2: Multiple Sources

```yaml
requestPlugins:
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
        - type: query
          name: api_key
        - type: cookie
          name: api_key
      secretGroups:
        - name: default
          secretRefs:
            - name: api-keys
```

### Scenario 3: Anonymous Access

```yaml
requestPlugins:
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      secretGroups:
        - name: default
          secretRefs:
            - name: api-keys
      anonymous: "guest"
```

### Scenario 4: Forward User Metadata Upstream

```yaml
requestPlugins:
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      secretGroups:
        - name: default
          secretRefs:
            - name: api-keys
      upstreamHeaderFields:
        - "X-Consumer-Username"
        - "X-Customer-ID"
        - "X-User-Tier"
```

### Scenario 5: Organize Keys by Group (Multi-tenant)

`secretGroups` lets you organize Secrets from different partners or tenants into named groups. The originating group of each resolved key is recorded in the access log for audit and attribution.

```yaml
requestPlugins:
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      realm: "Partner API"
      keyField: "key"
      secretGroups:
        - name: partner-a
          description: "Partner A API keys"
          secretRefs:
            - name: api-keys-partner-a
              namespace: edgion-default
        - name: partner-b
          description: "Partner B API keys"
          secretRefs:
            - name: api-keys-partner-b
              namespace: edgion-default
      upstreamHeaderFields:
        - "X-Consumer-Username"
        - "X-Customer-ID"
```

**Access log output**: On successful authentication, PluginLog records the group:
- With username header: `OK group=partner-a user <masked>`
- Without username header: `OK group=partner-a`

Failure branches (`Deny bad key`, `Deny no key`) and anonymous pass-through (`Allow anon`) intentionally omit `group=` to avoid leaking multi-tenant topology to unauthenticated callers.

---

## Behavior Details

- Multiple `keySources` are checked in order; the first non-empty value is used for authentication
- Every `upstreamHeaderFields` name is removed from the request on both the keyed and the
  anonymous path before the matched key's values are written, so a name the key has no value for
  stays absent instead of carrying a client-supplied value. If a header cannot be cleared or
  written, or a hidden header or query credential cannot be removed, the request is rejected
  with the normal `401` (after `authFailureDelayMs`); the access log records
  `identity header rejected` or `credential removal failed` and the diagnostic
  `AuthHeaderMutationRejected`. A hidden `cookie` credential that cannot be removed keeps the
  cookie cleanup response instead (`400` for an unsanitizable `Cookie` header, `500` for an
  internal failure), not the `401`.
  `AuthHeaderMutationRejected` is an access-log diagnostic (an `error` entry in the
  `diagnostics` array), not a metric, and one occurrence is not an alert condition by itself:
  most causes are operator misconfiguration, but a client can trigger it by sending the hidden
  query-parameter credential to a gRPC-JSON-transcoded route, whose query is sealed. Alert on
  its rate with log-based alerting.
  `upstreamHeaderFields` names must be valid header names and cannot be `Host` or a framing/hop-by-hop header (`Content-Length`, `Transfer-Encoding`, `TE`, `Trailer`, `Expect`, `Connection`, `Keep-Alive`, `Proxy-Connection`, `Upgrade`), since every
  listed name is removed on each request (including for later plugins in the chain, so do not list the
  header this plugin reads its key from). `anonymous` must be valid header text when it is forwarded as
  `X-Consumer-Username` (the whitelist entry matches case-insensitively).
- With `hideCredentials: true`, the source used for authentication is removed from the request before it is forwarded upstream. This covers the `header`, `query`, and `cookie` sources (for `query`, the matched parameter is stripped from the request URI; for `cookie`, the matched cookie is removed from the `Cookie` header). A `ctx` source (including a value written by [WebhookKeyGet](./webhook-key-get.md)) is already server-side and never travels in the forwarded request, so nothing is stripped for it. WebhookKeyGet must run **before** KeyAuth; a reversed `requestPlugins` list is a silent auth bypass.
- In `anonymous` mode, requests without a key still pass through with `X-Anonymous-Consumer: true`
- Keys are stored in the `keys.yaml` field of Kubernetes Secrets as a YAML array
- When the same raw key appears in multiple groups, the first group wins; a warning is logged with both group names

---

## Troubleshooting

### Problem 1: Always returns 401

**Cause**: Secret not configured correctly or key format is incorrect.

**Solution**:
```bash
kubectl get secret api-keys -o yaml
# Ensure keys.yaml field exists and format is correct
```

### Problem 2: Key doesn't match

**Cause**: `keyField` doesn't match the field name in Secret.

**Solution**: Ensure `keyField` configuration matches the field name in keys.yaml.

---

## Complete Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected-api
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: key-auth-plugin
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: key-auth-plugin
spec:
  requestPlugins:
      type: KeyAuth
      config:
        keySources:
          - type: header
            name: "X-API-Key"
          - type: query
            name: api_key
        secretGroups:
          - name: default
            secretRefs:
              - name: api-keys
        hideCredentials: true
        realm: "Protected API"
---
apiVersion: v1
kind: Secret
metadata:
  name: api-keys
type: Opaque
stringData:
  keys.yaml: |
    - key: "production-key-001"
      username: "service-a"
    - key: "production-key-002"
      username: "service-b"
```

## Related Docs

- [Basic Auth](./basic-auth.md)
- [JWT Auth](./jwt-auth.md)
- [HMAC Auth](./hmac-auth.md)

### Cookie credential cleanup

With `hideCredentials: true`, successful Cookie authentication removes every
occurrence of the selected cookie name across all Cookie header fields. Surviving
names and values are preserved; separators may be normalized and fields combined.
Subsequent plugins, webhooks and upstream requests observe the updated header.
If removal encounters non-UTF-8 bytes, the request fails with HTTP 400; an internal
update failure returns HTTP 500. Neither failure forwards the unsanitized request.
Without a Cookie edit, the raw Cookie fields are not reassembled.
