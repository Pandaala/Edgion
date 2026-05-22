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
        secretGroups:
          - name: default
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
| `secretGroups` | Array | Yes* | none | Named groups of credential Secrets (see sub-table below) |
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

### `secretGroups` sub-table (`HmacAuthSecretGroup`)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Group name (Kubernetes label-value charset, max 63 chars) |
| `description` | String | No | Human-readable description (not used at runtime) |
| `secretRefs` | Array | Yes | Kubernetes Secret references for this group (at least 1) |

---

## Usage Scenarios

### Scenario 1: Basic HMAC Authentication

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretGroups:
        - name: default
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
      secretGroups:
        - name: default
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
      secretGroups:
        - name: default
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

## Replay-Protection Security Model

HMAC Auth uses a **timestamp window** to prevent signature replay — the request `Date` / `X-Date` header must be within `clockSkew` (default `300` seconds / 5 minutes) of the server clock. **The plugin does NOT perform nonce deduplication.**

This matches the dominant industry pattern for HMAC authentication (AWS SigV4, Stripe Webhooks, GitHub Webhooks, Kong `hmac-auth`, APISIX `hmac-auth`) — "timestamp window + HTTPS", no gateway-layer nonce storage.

### In-Window Replay Risk and Mitigations

In theory, an attacker who obtains a valid signature can replay it arbitrarily many times within `clockSkew`. The **prerequisite** is obtaining the signature, and in a correctly deployed setup several layers block that:

| Threat path | Defense |
|---|---|
| Man-in-the-middle capture | **HTTPS** (TLS prevents signature exposure on the wire) |
| Signature leaked via logs | Enable `hideCredentials: true`; do not add `Authorization` / `Signature` to access-log headers |
| Legitimate client replays maliciously | **Business-layer idempotency keys** (not a gateway concern) |
| Window configured too wide | Shrink `clockSkew` (e.g., `60`) on high-sensitivity endpoints |

### Recommended Configuration

**Minimum secure production config**:

```yaml
filters:
  - type: HmacAuth
    config:
      secretGroups:
        - name: default
          secretRefs:
            - name: hmac-creds
      clockSkew: 300          # 5-minute window; shrink to 60 for sensitive endpoints
      hideCredentials: true   # strip Authorization/Signature from upstream (reduces log-leak surface)
      enforceHeaders:
        - date                # timestamp must be signed
        - host
      # Business-layer idempotency: sensitive non-idempotent ops (payments, password changes)
      # must be protected by the application via an Idempotency-Key header.
```

**Also required**:
- **HTTPS is mandatory**: HMAC signatures can be captured on cleartext; HTTPS is the baseline assumption of this plugin
- **Do not log signatures**: default off, but avoid adding `Authorization` / `Signature` to `accessLog.extraHeaders`
- **Idempotency for critical ops**: For non-idempotent, high-sensitivity operations (payment, password change, transfer), the backend should require an `Idempotency-Key` header and dedupe at the business layer — this is the correct place for replay protection

### Why Doesn't Edgion Do Nonce Dedup at the Gateway?

Nonce deduplication requires **per-request state storage** (in-memory LRU or Redis), each with problems:

- **In-memory LRU**: State is per-gateway-instance; a multi-gateway deployment fails because an attacker can switch instances and replay — **illusory defense**
- **Redis backend**: Adds ~1–3ms per request and a hard dependency on Redis; the "fail-open vs fail-close on Redis failure" question is a dilemma either way

The industry (AWS / Stripe / GitHub / Kong / APISIX) has settled on "gateway does timestamp window, business layer does idempotency keys". Edgion follows the same layering.

If your deployment truly needs gateway-layer nonce deduplication (edge cases), please open an issue to discuss a standalone implementation.

---

## Organizing Secrets by Group

`secretGroups` lets you manage credentials for multiple partners or tenants in
separate named groups. Each group independently maintains its own Secret
references. When a request is successfully authenticated, the access log
records the originating group (`OK group=<name> user <X>`), providing a clear
audit trail.

**Cross-group duplicate usernames**: If the same username appears in more than
one group, the **first-wins** rule applies — the credential from the
earliest-listed group is kept, and a `warn` log is emitted for the rejected
duplicate. The plugin continues serving requests normally.

```yaml
requestPlugins:
  - enable: true
    type: HmacAuth
    config:
      secretGroups:
        - name: partner-a
          description: "Partner A API credentials"
          secretRefs:
            - name: hmac-credentials-partner-a
              namespace: edgion-default
        - name: partner-b
          description: "Partner B API credentials"
          secretRefs:
            - name: hmac-credentials-partner-b
              namespace: edgion-default
      algorithms:
        - hmac-sha256
      clockSkew: 300
      enforceHeaders:
        - "@request-target"
        - host
        - date
      upstreamHeaderFields:
        - X-Consumer-Username
        - X-Customer-ID
```

On successful authentication, the access log records:

```
OK group=partner-a user partner-a-user
OK group=partner-b user partner-b-user
```

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
