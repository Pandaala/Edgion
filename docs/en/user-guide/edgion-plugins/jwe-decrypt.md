# JWE Decrypt Plugin

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

## Configuration Parameters (Phase 1)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `secretRef` | object | - | Reference to a K8s Secret containing a `secret` field |
| `keyManagementAlgorithm` | enum | `Dir` | Key management algorithm (currently `Dir` only) |
| `contentEncryptionAlgorithm` | enum | `A256GCM` | Content encryption algorithm (currently `A256GCM` only) |
| `header` | string | `authorization` | Request header to read the JWE from |
| `forwardHeader` | string | `authorization` | Request header to write the decrypted plaintext to |
| `stripPrefix` | string | - | Prefix to strip before extracting the token (e.g., `Bearer `) |
| `strict` | bool | `true` | Whether to reject when no token is present |
| `hideCredentials` | bool | `false` | Whether to remove the original credential header |
| `maxTokenSize` | integer | `8192` | Maximum token length (bytes) |
| `allowedAlgorithms` | enum[] | - | `enc` allowlist |
| `payloadToHeaders` | map | - | Map decrypted payload fields to upstream headers |
| `maxHeaderValueBytes` | integer | `4096` | Maximum size per mapped header value |
| `maxTotalHeaderBytes` | integer | `16384` | Maximum total size for all mapped headers |
| `storePayloadInCtx` | bool | `false` | Whether to store in ctx variable `jwe_payload` |
| `authFailureDelayMs` | integer | `0` | Failure delay in milliseconds |

## Example Configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwe-decrypt-test
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: JweDecrypt
      config:
        secretRef:
          name: jwe-secret
        keyManagementAlgorithm: Dir
        contentEncryptionAlgorithm: A256GCM
        header: authorization
        forwardHeader: x-decrypted-auth
        stripPrefix: "Bearer "
        strict: true
        hideCredentials: true
        allowedAlgorithms: [A256GCM]
        payloadToHeaders:
          uid: x-user-id
          user.department: x-user-dept
          permissions.admin: x-is-admin
```

Corresponding Secret:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwe-secret
  namespace: default
type: Opaque
data:
  secret: MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY= # base64 of a 32-byte key
```

## Error Semantics

| Scenario | Status Code | PluginLog |
|----------|------------|-----------|
| Missing JWE and `strict=true` | `403` | `jwe:no-token` |
| Invalid JWE format | `400` | `jwe:invalid-format` |
| JWE missing required headers (`alg`/`enc`) | `400` | `jwe:missing-header` |
| Secret/key unavailable | `403` | `jwe:no-key` |
| Key length doesn't match algorithm | `500` | `jwe:key-len-err` |
| Decryption failed | `403` | `jwe:decrypt-fail` |
| Unsupported or mismatched algorithm | `400` | `jwe:bad-alg` |
| Algorithm not in allowlist | `400` | `jwe:alg-denied` |
| Token exceeds size limit | `400` | `jwe:too-large` |
