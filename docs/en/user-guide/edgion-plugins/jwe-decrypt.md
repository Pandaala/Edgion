# JWE Decrypt Plugin

> **🔌 Edgion extension**
>
> JweDecrypt is a JWE decryption authentication plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

`JweDecrypt` decrypts Compact JWE tokens from request headers during the request phase (currently supporting `alg=dir` + `enc=A256GCM`), and passes the decrypted plaintext to the upstream service.

Use cases:
- Clients use JWE to transmit encrypted identity payloads
- The gateway performs unified decryption, so backends only consume plaintext or mapped identity headers
- Standard authentication failure responses similar to `KeyAuth` / `JwtAuth` are needed

## Features

- Reuses common authentication capabilities:
  - `send_auth_error_response()` for unified error responses
  - `set_claims_headers()` for payload field-to-header mapping (with injection protection and size limits)
- Supports strict mode (`strict`) and bypass mode
- Supports lazy key loading (loads Secret on first request)
- Supports `payloadToHeaders` dot-notation paths (e.g., `user.department`)
- Supports failure delay (`authFailureDelayMs`) to reduce timing side-channel risks
- Supports `base64Secret` for double-base64-encoded keys

## Quick Start

Create the decryption key Secret, then wire it to a plugin and an HTTPRoute:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwe-key-secret
  namespace: production
type: Opaque
data:
  secret: MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=  # base64 of a 32-byte key
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-plugin
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        strict: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
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
            name: jwe-decrypt-plugin
      backendRefs:
        - name: api-service
          port: 8080
```

> With only `secretRef` specified, all other fields use their defaults: reads the `authorization` header, writes the decrypted payload back to the same `authorization` header, strict mode enabled, no prefix stripping.

## Configuration Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `secretRef` | Object | **Required** | Reference to a K8s Secret containing a `secret` field |
| `keyManagementAlgorithm` | Enum | `Dir` | Key management algorithm (currently `Dir` only) |
| `contentEncryptionAlgorithm` | Enum | `A256GCM` | Content encryption algorithm (currently `A256GCM` only) |
| `header` | String | `authorization` | Request header to read the JWE token from |
| `forwardHeader` | String | `authorization` | Request header to write the decrypted plaintext to (upstream receives this) |
| `stripPrefix` | String | none | Prefix to strip before extracting the token (e.g., `Bearer `) |
| `strict` | Boolean | `true` | Reject the request when no token is present; set to `false` to allow anonymous pass-through |
| `hideCredentials` | Boolean | `false` | Remove the original credential header before proxying upstream |
| `base64Secret` | Boolean | `false` | Decode the Secret value as base64 one additional time (for double-encoded keys) |
| `maxTokenSize` | Integer | `8192` | Maximum token length in bytes |
| `allowedAlgorithms` | Array\<Enum\> | none | Content encryption algorithm allowlist; must include `contentEncryptionAlgorithm` if set |
| `payloadToHeaders` | Object | none | Map decrypted JSON payload fields to upstream request headers (supports dot-path, e.g. `user.department`) |
| `maxHeaderValueBytes` | Integer | `4096` | Maximum size per mapped header value |
| `maxTotalHeaderBytes` | Integer | `16384` | Maximum total size for all mapped headers |
| `storePayloadInCtx` | Boolean | `false` | Store the decrypted payload in the `jwe_payload` context variable (readable by downstream plugins; excluded from access log) |
| `authFailureDelayMs` | Integer | `0` | Delay in milliseconds before sending an auth failure response (reduces timing side-channels) |

## Common Configuration Scenarios

### 1. Decrypt and forward to a different header

Use a dedicated `x-decrypted-auth` header for the upstream instead of overwriting `authorization`.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-forward
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        header: authorization
        forwardHeader: x-decrypted-auth
        stripPrefix: "Bearer "
        hideCredentials: true
```

### 2. Map payload fields to upstream headers

Unpack specific fields from the decrypted JSON payload and inject them as individual upstream headers.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-claims
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        stripPrefix: "Bearer "
        hideCredentials: true
        payloadToHeaders:
          uid: x-user-id
          user.department: x-user-dept
          permissions.admin: x-is-admin
        maxHeaderValueBytes: 4096
        maxTotalHeaderBytes: 16384
```

### 3. Lenient mode with timing protection

Allow requests without a JWE token (anonymous pass-through) and add a 200 ms delay on auth failures to reduce timing-based enumeration.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-lenient
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        strict: false
        authFailureDelayMs: 200
```

### 4. Store payload in context for downstream plugins

Keep the full decrypted payload available to downstream plugins (e.g., DSL or CtxSet) via the `jwe_payload` context variable, without exposing it in access logs.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-ctx
  namespace: production
spec:
  requestPlugins:
    - type: JweDecrypt
      config:
        secretRef:
          name: jwe-key-secret
        storePayloadInCtx: true
        strict: true
```

## Error Semantics

| Scenario | Status Code | PluginLog |
|----------|------------|-----------|
| Missing JWE and `strict=true` | `403` | `jwe:no-token` |
| Invalid JWE format | `400` | `jwe:invalid-format` |
| JWE missing required headers (`alg`/`enc`) | `400` | `jwe:missing-header` |
| Secret/key unavailable | `403` | `jwe:no-key` |
| Key length does not match algorithm | `500` | `jwe:key-len-err` |
| Decryption failed | `403` | `jwe:decrypt-fail` |
| Unsupported or mismatched algorithm | `400` | `jwe:bad-alg` |
| Algorithm not in allowlist | `400` | `jwe:alg-denied` |
| Token exceeds size limit | `400` | `jwe:too-large` |

## Related Links

- [JWE Decrypt — HTTPRoute filter view](../http-route/filters/edgion-plugins/jwe-decrypt.md) - Using JweDecrypt within an HTTPRoute filter
- [JwtAuth Plugin](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT token authentication
- [OpenID Connect Plugin](openid-connect.md) - OIDC/OAuth2 authentication
- [ForwardAuth Plugin](forward-auth.md) - External authentication plugin
- [CtxSet Plugin](ctx-setter.md) - Context variable plugin; can read `jwe_payload` set by `storePayloadInCtx`
