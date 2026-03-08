# Key Auth Plugin

> **🔌 Edgion Extension**
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
  namespace: default
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
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: KeyAuth
      config:
        keySources:
          - type: header
            name: "X-API-Key"
        secretRefs:
          - name: api-keys
```

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `keySources` | Array | No | `[{type:"header",name:"apikey"}, {type:"query",name:"apikey"}]` | Key source list |
| `hideCredentials` | Boolean | No | `false` | Remove API Key from request after validation |
| `authFailureDelayMs` | Integer | No | `0` | Delay response on auth failure (brute force protection) |
| `anonymous` | String | No | none | Anonymous username; unauthenticated requests pass through |
| `realm` | String | No | `"API Gateway"` | Authentication realm name |
| `keyField` | String | No | `"key"` | Field name storing the key value in Secret |
| `secretRefs` | Array | Yes | none | Kubernetes Secret references |
| `upstreamHeaderFields` | Array | No | `[]` | Extra headers to forward upstream (from key metadata) |

### KeySource Sub-fields

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `type` | String | Yes | Source type: `header` / `query` / `cookie` / `ctx` |
| `name` | String | Yes | Source name (header name, query param name, etc.) |

---

## Usage Scenarios

### Scenario 1: Key from Header

```yaml
requestPlugins:
  - enable: true
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
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
  - enable: true
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
        - type: query
          name: api_key
        - type: cookie
          name: api_key
      secretRefs:
        - name: api-keys
```

### Scenario 3: Anonymous Access

```yaml
requestPlugins:
  - enable: true
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      secretRefs:
        - name: api-keys
      anonymous: "guest"
```

### Scenario 4: Forward User Metadata Upstream

```yaml
requestPlugins:
  - enable: true
    type: KeyAuth
    config:
      keySources:
        - type: header
          name: "X-API-Key"
      secretRefs:
        - name: api-keys
      upstreamHeaderFields:
        - "X-Consumer-Username"
        - "X-Customer-ID"
        - "X-User-Tier"
```

---

## Behavior Details

- Multiple `keySources` are checked in order; the first non-empty value is used for authentication
- With `hideCredentials: true`, the source used for authentication is removed from the request
- In `anonymous` mode, requests without a key still pass through with `X-Anonymous-Consumer: true`
- Keys are stored in the `keys.yaml` field of Kubernetes Secrets as a YAML array

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
    - name: my-gateway
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
    - enable: true
      type: KeyAuth
      config:
        keySources:
          - type: header
            name: "X-API-Key"
          - type: query
            name: api_key
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
