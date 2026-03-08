# HTTP to HTTPS Redirect Guide

> **🔌 Edgion Extension**
> 
> This feature is implemented through Edgion custom Annotations and is not part of the standard Gateway API specification.

This guide explains how to enable global HTTP to HTTPS redirection via Gateway annotations.

## Feature Overview

When this feature is enabled, all requests sent to HTTP ports will be automatically redirected to HTTPS, similar to the following nginx configuration:

```nginx
return 301 https://$host$request_uri;
```

## Quick Start

### Enable Redirect

Add an annotation to the Gateway resource:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  namespace: default
  annotations:
    edgion.io/http-to-https-redirect: "true"
spec:
  gatewayClassName: edgion
  listeners:
    # HTTP listener - will automatically redirect to HTTPS
    - name: http
      port: 80
      protocol: HTTP
    # HTTPS listener - handles requests normally
    - name: https
      port: 443
      protocol: HTTPS
      tls:
        certificateRefs:
          - name: my-tls-secret
```

### Custom HTTPS Port

If the HTTPS service runs on a non-standard port, you can specify the redirect target port:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  annotations:
    edgion.io/http-to-https-redirect: "true"
    edgion.io/https-redirect-port: "8443"
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 8080
      protocol: HTTP
    - name: https
      port: 8443
      protocol: HTTPS
      tls:
        certificateRefs:
          - name: my-tls-secret
```

## Annotation Reference

> **🔌 Edgion Extension Annotations**
> 
> The following Annotations use the `edgion.io/` prefix and are extension configurations for Edgion Gateway.

| Annotation | Type | Default | Description |
|------------|------|---------|-------------|
| `edgion.io/http-to-https-redirect` | string | `"false"` | Set to `"true"` to enable HTTP to HTTPS redirect |
| `edgion.io/https-redirect-port` | string | `"443"` | HTTPS redirect target port |

## How It Works

1. When the Gateway is configured with `edgion.io/http-to-https-redirect: "true"`
2. All HTTP protocol listeners use a lightweight redirect handler
3. Upon receiving a request, it immediately returns a `301 Moved Permanently` response
4. The `Location` header is set to the corresponding HTTPS URL

### Example Request/Response

**Request:**
```
GET /api/users?page=1 HTTP/1.1
Host: example.com
```

**Response:**
```
HTTP/1.1 301 Moved Permanently
Location: https://example.com/api/users?page=1
Content-Length: 0
Connection: close
```

## Notes

1. **Only affects HTTP listeners**: This annotation only applies to listeners with `protocol: HTTP`. HTTPS listeners are not affected

2. **Gateway-wide**: Once enabled, all HTTP listeners under the Gateway will redirect. Per-listener configuration is not supported

3. **No business logic**: The redirect occurs at the earliest stage of request processing, without executing any plugins or route matching

4. **SEO friendly**: Uses 301 permanent redirect, so search engines will automatically update their indexes

## FAQ

### Q: How to enable redirect only for specific paths?

A: This feature provides global redirection and does not support path-level control. For finer-grained control, configure the RequestRedirect filter in HTTPRoute.

### Q: Does the redirect preserve query parameters?

A: Yes, the complete URI (including path and query parameters) is preserved.

### Q: What is the performance impact?

A: The redirect handler is very lightweight and does not involve any upstream connections or complex route matching, resulting in minimal performance overhead.

## Related Features

- [Gateway Overview](./overview.md)
- [HTTPS Listener](./listeners/https.md)
- [TLS Termination](./tls/tls-termination.md)
