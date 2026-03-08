# HMAC Auth Plugin

> **🔌 Edgion Extension**
> 
> HmacAuth is an HMAC signature authentication plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

HMAC Auth verifies request signatures based on the HTTP Signature specification, ensuring request integrity and identity authenticity. Supports hmac-sha256, hmac-sha384, and hmac-sha512 algorithms.

**How it works**:
1. Client computes an HMAC signature over specified parts of the request using a shared secret
2. Signature information is placed in the `Authorization` or `Signature` header
3. Plugin recomputes the signature and compares it against the request
4. Matching signature within time window: access granted
5. Mismatched or expired signature: returns 401

## Quick Start

### Create Credential Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: hmac-credentials
type: Opaque
stringData:
  username: "service-a"
  secret: "a-very-long-secret-key-at-least-32-chars!"
```

### Configure the Plugin

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: hmac-auth-plugin
spec:
  requestPlugins:
    - enable: true
      type: HmacAuth
      config:
        secretRefs:
          - name: hmac-credentials
        algorithms:
          - hmac-sha256
        clockSkew: 300
```

---

## Configuration Reference

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `secretRefs` | Array | Yes* | none | Kubernetes Secret references |
| `algorithms` | Array | No | `["hmac-sha256","hmac-sha384","hmac-sha512"]` | Allowed signing algorithms |
| `clockSkew` | Integer | No | `300` | Allowed clock skew in seconds |
| `enforceHeaders` | Array | No | none | Headers that must be included in signature |
| `validateRequestBody` | Boolean | No | `false` | Whether to validate request body Digest |
| `hideCredentials` | Boolean | No | `false` | Remove authentication headers |
| `anonymous` | String | No | none | Anonymous username |
| `realm` | String | No | `"edgion"` | Authentication realm |
| `authFailureDelayMs` | Integer | No | `0` | Auth failure response delay (ms) |
| `minSecretLength` | Integer | No | `32` | Minimum secret key length |
| `secretField` | String | No | `"secret"` | Secret field name in K8s Secret |
| `usernameField` | String | No | `"username"` | Username field name in K8s Secret |
| `upstreamHeaderFields` | Array | No | `[]` | Extra headers to forward upstream |

\* Required when `anonymous` is not set.

---

## Usage Scenarios

### Scenario 1: Basic HMAC Authentication

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretRefs:
        - name: hmac-credentials
      algorithms:
        - hmac-sha256
      hideCredentials: true
```

### Scenario 2: Enforce Specific Headers in Signature

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretRefs:
        - name: hmac-credentials
      enforceHeaders:
        - "@request-target"
        - host
        - date
        - content-type
      validateRequestBody: true
```

### Scenario 3: Multi-User Authentication

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretRefs:
        - name: user-a-credentials
        - name: user-b-credentials
      algorithms:
        - hmac-sha256
        - hmac-sha512
      clockSkew: 600
      upstreamHeaderFields:
        - X-Consumer-Username
```

---

## Signature Format

Client requests must include the following header:

```
Authorization: Signature keyId="service-a",algorithm="hmac-sha256",headers="@request-target host date",signature="base64-encoded-signature"
Date: Mon, 08 Mar 2026 10:00:00 GMT
```

### Signature Computation Steps

1. Build the signing string by concatenating header values in the order specified by `headers`
2. Compute HMAC signature using the algorithm and shared secret
3. Base64-encode the result

---

## Troubleshooting

### Problem 1: Signature Verification Fails

**Cause**: Clock skew too large or incorrect signature computation.

**Solution**:
- Ensure client and server clocks are synchronized
- Increase `clockSkew` value
- Verify the signing string construction

### Problem 2: Secret Length Too Short

**Cause**: Secret key shorter than `minSecretLength`.

**Solution**: Use a key of at least 32 characters, or adjust `minSecretLength`.

---

## Related Docs

- [Basic Auth](./basic-auth.md)
- [Key Auth](./key-auth.md)
- [JWT Auth](./jwt-auth.md)
