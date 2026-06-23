# HTTP to HTTPS Redirect

Automatically redirect plaintext HTTP requests to their HTTPS equivalent.

> **🔌 Edgion extension**
>
> This redirect is configured through the `edgion.io/http-to-https-redirect` Gateway annotation; it is not part of standard Gateway API.

## Overview

Set the `edgion.io/http-to-https-redirect` annotation on a Gateway to make Edgion answer
plaintext HTTP requests with a `301 Moved Permanently` redirect to the matching `https://`
URL. This is the common way to enforce TLS without dropping clients that still connect over
port 80. The redirect preserves the full path and query string and runs at the earliest stage
of request processing — no plugins or route matching execute.

The annotation accepts `"true"` or `"false"`; the default is `"false"`, in which case HTTP
requests route normally.

## Configuration

Apply the annotation at the Gateway level. It covers every `HTTP` listener on the Gateway:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: edgion-system
  annotations:
    edgion.io/http-to-https-redirect: "true"
spec:
  gatewayClassName: edgion
  listeners:
    # HTTP listener: requests are redirected to HTTPS
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
    # HTTPS listener: serves requests normally
    - name: https
      port: 443
      protocol: HTTPS
      hostname: app.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: app-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
```

## Annotation reference

| Annotation | Type | Default | Description |
|------------|------|---------|-------------|
| `edgion.io/http-to-https-redirect` | String | `"false"` | When `"true"`, every `HTTP` listener on the Gateway returns a `301` redirect to the `https://` equivalent. |
| `edgion.io/https-redirect-port` | String | `"443"` | Target port for the HTTPS redirect, used when the HTTPS service runs on a non-standard port. |

## Scenarios

### Redirect all HTTP traffic to standard HTTPS

The annotation turns every `HTTP` listener into a redirect to port 443.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: site-gateway
  namespace: edgion-system
  annotations:
    edgion.io/http-to-https-redirect: "true"
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
    - name: https
      port: 443
      protocol: HTTPS
      hostname: app.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: app-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
```

### Redirect to a non-standard HTTPS port

When HTTPS is served on a custom port, set `edgion.io/https-redirect-port` so the `Location`
header points at the right port.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: custom-port-gateway
  namespace: edgion-system
  annotations:
    edgion.io/http-to-https-redirect: "true"
    edgion.io/https-redirect-port: "8443"
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 8080
      protocol: HTTP
      allowedRoutes:
        namespaces:
          from: Same
    - name: https
      port: 8443
      protocol: HTTPS
      hostname: app.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - name: app-example-com-tls
      allowedRoutes:
        namespaces:
          from: Same
```

### Per-path redirect with an HTTPRoute filter

The Gateway annotation is all-or-nothing across HTTP listeners. For path-level control, leave
the annotation off and use a `RequestRedirect` filter on an HTTPRoute instead.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: redirect-route
  namespace: edgion-system
spec:
  parentRefs:
    - name: site-gateway
      sectionName: http
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /secure/
      filters:
        - type: RequestRedirect
          requestRedirect:
            scheme: https
            statusCode: 301
```

## Edge cases and caveats

- **Quote the value**: write `"true"` so YAML does not parse it as a boolean.
- **Gateway-wide, not per-listener**: once enabled, *all* `HTTP` listeners on the Gateway
  redirect. There is no per-listener override; use an HTTPRoute `RequestRedirect` filter for
  finer control.
- **HTTP listeners only**: the annotation affects only `protocol: HTTP` listeners. `HTTPS`,
  `TLS`, `TCP`, and `UDP` listeners are unaffected.
- **301 is cached**: browsers cache the permanent redirect, so test with a fresh client when
  iterating.
- **Provide a matching HTTPS listener**: the redirect targets `https://<host>` (on
  `https-redirect-port`); make sure something is listening there or clients hit a dead port.
- **Query and path preserved**: the full URI, including query parameters, is carried into the
  `Location` header.

## Related documents

- [HTTPS listener](listeners/https.md)
- [HTTP listener](listeners/http.md)
- [Preflight policy](preflight-policy.md)
- [Gateway overview](overview.md)
