# HMAC Auth Plugin

> **🔌 Edgion extension**
> 
> HmacAuth is an HMAC signature authentication plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

HMAC Auth verifies a shared-secret signature over selected parts of the request so the gateway can authenticate the caller and detect tampering. It supports hmac-sha256, hmac-sha384, and hmac-sha512.

This is Edgion's request contract. Other gateways use different header encodings for the same class of feature; clients must follow the format below.

**How it works**:
1. Client computes an HMAC over the declared request parts using a shared secret
2. Client sends the result in `Authorization`, or in `Proxy-Authorization` when `Authorization` is absent
3. Plugin recomputes the signature and compares it against the request
4. Matching signature within the time window: access granted
5. Mismatched or expired signature: returns 401

---

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
    - type: HmacAuth
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

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `secretGroups` | Array\<Object\> | **Required** (unless `anonymous` is set) | Named groups of credential Secrets (see sub-table below) |
| `algorithms` | Array\<String\> | `["hmac-sha256","hmac-sha384","hmac-sha512"]` | Allowed signing algorithms |
| `clockSkew` | Integer | `300` | Allowed clock skew in seconds |
| `enforceHeaders` | Array\<String\> | none | Headers that must be included in signature |
| `validateRequestBody` | Boolean | `false` | Whether to validate request body Digest |
| `hideCredentials` | Boolean | `false` | After successful authentication, remove `Authorization`, `Proxy-Authorization`, and `Signature` before proxying. `Signature` is stripped only; it is not an authentication source. Anonymous requests that skip authentication do not run this cleanup |
| `anonymous` | String | none | Anonymous username |
| `realm` | String | `"edgion"` | Authentication realm; [Character rules](README.md#authentication-realm) |
| `authFailureDelayMs` | Integer | `0` | Auth failure response delay (ms) |
| `minSecretLength` | Integer | `32` | Minimum secret key length |
| `secretField` | String | `"secret"` | Secret field name in K8s Secret |
| `usernameField` | String | `"username"` | Username field name in K8s Secret |
| `upstreamHeaderFields` | Array\<String\> | `[]` | Extra headers to forward upstream |

### `secretGroups` sub-table (`HmacAuthSecretGroup`)

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Group name (Kubernetes label-value charset, max 63 chars) |
| `description` | String | none | Human-readable description (not used at runtime) |
| `secretRefs` | Array\<Object\> | **Required** | Kubernetes Secret references for this group (at least 1) |

---

## Usage Scenarios

### Scenario 1: Basic HMAC Authentication

```yaml
requestPlugins:
  - type: HmacAuth
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
  - type: HmacAuth
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
  - type: HmacAuth
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

## Request Format

The client must send credentials in `Authorization`. If that header is absent, the plugin reads `Proxy-Authorization`. A standalone `Signature` header is not used for authentication.

```
Authorization: hmac username="service-a", algorithm="hmac-sha256", headers="@request-target host date", signature="<base64-signature>"
Date: Sat, 05 Sep 2026 14:00:00 GMT
```

Required quoted parameters:

| Parameter | Meaning |
|-----------|---------|
| `username` | Credential username (looked up in `secretGroups`) |
| `algorithm` | One of `hmac-sha256`, `hmac-sha384`, `hmac-sha512` |
| `headers` | Space-separated names that enter the signing string, in this order |
| `signature` | Standard Base64 of the HMAC digest |

The signed name list must include `date` or `x-date`. Send a current `Date` or `X-Date` value in RFC 2822 or RFC 3339 form. The sample date above shows a valid RFC 2822 weekday; replace it with the current request time. The gateway rejects timestamps outside `clockSkew`.

### Signing String

Build one line per name in the `headers` list, in that order, and join them with `\n` (no trailing newline). The plugin lowercases every name in `headers` before building the string, so the left-hand side is always lowercase:

```
<lowercase-name>: <value>
```

- Ordinary names use the request header value as received.
- `@request-target` and `(request-target)` use the lowercase method, a space, the path, and `?` plus the query when a query is present:

```
@request-target: get /v1/items?page=1
host: api.example.com
date: Sat, 05 Sep 2026 14:00:00 GMT
```

Then HMAC that string with the shared secret and Base64-encode the digest.

When `validateRequestBody` is true, also sign `digest` and send `Digest: SHA-256=<base64(sha256(body))>`.

---

## Replay-Protection Security Model

HMAC Auth uses a **timestamp window** to prevent signature replay — the request `Date` / `X-Date` header must be within `clockSkew` (default `300` seconds / 5 minutes) of the server clock. **The plugin does NOT perform nonce deduplication.**

This matches a common HMAC authentication pattern (AWS SigV4, Stripe Webhooks, and typical API-gateway HMAC plugins) — "timestamp window + HTTPS", no gateway-layer nonce storage.

### In-Window Replay Risk and Mitigations

In theory, an attacker who obtains a valid signature can replay it arbitrarily many times within `clockSkew`. The **prerequisite** is obtaining the signature, and in a correctly deployed setup several layers block that:

| Threat path | Defense |
|---|---|
| Man-in-the-middle capture | **HTTPS** (TLS prevents signature exposure on the wire) |
| Signature leaked via logs | Enable `hideCredentials: true`; do not add `Authorization`, `Proxy-Authorization`, or `Signature` to access-log headers |
| Legitimate client replays maliciously | **Business-layer idempotency keys** (not a gateway concern) |
| Window configured too wide | Shrink `clockSkew` (e.g., `60`) on high-sensitivity endpoints |

### Recommended Configuration

**Minimum secure production config**:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: hmac-auth-secure
spec:
  requestPlugins:
    - type: HmacAuth
      config:
        secretGroups:
          - name: default
            secretRefs:
              - name: hmac-creds
        clockSkew: 300          # 5-minute window; shrink to 60 for sensitive endpoints
        hideCredentials: true   # strip Authorization, Proxy-Authorization, and Signature before upstream
        enforceHeaders:
          - date                # timestamp must be signed
          - host
        # Business-layer idempotency: sensitive non-idempotent ops (payments, password changes)
        # must be protected by the application via an Idempotency-Key header.
```

**Also required**:
- **HTTPS is mandatory**: HMAC signatures can be captured on cleartext; HTTPS is the baseline assumption of this plugin
- **Do not log signatures**: default off, but avoid adding `Authorization`, `Proxy-Authorization`, or `Signature` to `accessLog.extraHeaders`
- **Idempotency for critical ops**: For non-idempotent, high-sensitivity operations (payment, password change, transfer), the backend should require an `Idempotency-Key` header and dedupe at the business layer — this is the correct place for replay protection

### Why Doesn't Edgion Do Nonce Dedup at the Gateway?

Nonce deduplication requires **per-request state storage** (in-memory LRU or Redis), each with problems:

- **In-memory LRU**: State is per-gateway-instance; a multi-gateway deployment fails because an attacker can switch instances and replay — **illusory defense**
- **Redis backend**: Adds ~1–3ms per request and a hard dependency on Redis; the "fail-open vs fail-close on Redis failure" question is a dilemma either way

Edgion follows the common gateway layering: the edge enforces a timestamp window, and the application owns idempotency keys.

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

Controller duplicate warnings identify the rejected Secret (`secret_key`) and
the relevant groups (`existing_group` / `incoming_group`, or `group` within one
group). Short-secret warnings include `secret_len` and `min_len` alongside the
Secret and group. These warnings omit raw usernames and shared secret values;
use the resource pointers to inspect credentials through authorized Secret access.

```yaml
requestPlugins:
  - type: HmacAuth
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
- Confirm the request uses `hmac` with a `username` parameter, not a `Signature` / `keyId` header

### Problem 2: Secret Length Too Short

**Cause**: Secret key shorter than `minSecretLength`.

**Solution**: Use a key of at least 32 characters, or adjust `minSecretLength`.

---

## Related Docs

- [Basic Auth](./basic-auth.md)
- [Key Auth](./key-auth.md)
- [JWT Auth](./jwt-auth.md)
