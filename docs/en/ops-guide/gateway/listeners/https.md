# HTTPS Listener

Configure an HTTPS protocol listener to enable TLS termination.

## Basic Configuration

```yaml
listeners:
  - name: https
    port: 443
    protocol: HTTPS
    tls:
      mode: Terminate
      certificateRefs:
        - name: tls-secret
```

## Configuration Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | string | Yes | Listener name |
| port | int | Yes | Listening port |
| protocol | string | Yes | Protocol (HTTPS) |
| tls | object | Yes | TLS configuration |
| hostname | string | | Hostname filter |

### TLS Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| mode | string | | Terminate (default) or Passthrough |
| certificateRefs | array | Yes | Certificate Secret references |
| options | map | | TLS options |

## TLS Modes

### Terminate - TLS Termination

The Gateway decrypts TLS and forwards plaintext to the backend:

```yaml
tls:
  mode: Terminate
  certificateRefs:
    - name: tls-secret
```

### Passthrough - TLS Passthrough

The Gateway does not decrypt and forwards directly to the backend:

```yaml
tls:
  mode: Passthrough
```

## Certificate Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: tls-secret
type: kubernetes.io/tls
data:
  tls.crt: <base64-encoded-cert>
  tls.key: <base64-encoded-key>
```

## Examples

### Example 1: Basic HTTPS

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: https-gateway
spec:
  gatewayClassName: edgion
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        certificateRefs:
          - name: wildcard-tls
```

### Example 2: Multi-Domain Certificates

```yaml
listeners:
  - name: https-api
    port: 443
    protocol: HTTPS
    hostname: "api.example.com"
    tls:
      certificateRefs:
        - name: api-tls
  - name: https-web
    port: 443
    protocol: HTTPS
    hostname: "www.example.com"
    tls:
      certificateRefs:
        - name: web-tls
```

### Example 3: HTTP Auto-Redirect to HTTPS

```yaml
listeners:
  - name: http
    port: 80
    protocol: HTTP
  - name: https
    port: 443
    protocol: HTTPS
    tls:
      certificateRefs:
        - name: tls-secret
```

Use HTTPRoute's RequestRedirect filter to implement the redirect.

## Related Documentation

- [HTTP Listener](./http.md)
- [TLS Termination](../tls/tls-termination.md)
- [EdgionTls Extension](../tls/edgion-tls.md)
