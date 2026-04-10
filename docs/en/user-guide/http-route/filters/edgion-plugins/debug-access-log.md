# DebugAccessLogToHeader Plugin

> **🔌 Edgion Extension**
>  
> DebugAccessLogToHeader is a debug plugin provided by the `EdgionPlugins` CRD that serializes the current request's full access log as a JSON string and injects it into a response header. It is not part of the standard Gateway API.

> ⚠️ **Development/Test Environments Only**
>  
> This plugin exposes sensitive data such as IP addresses, routing information, and request metadata in HTTP response headers. **Never use it in production.**

## What is DebugAccessLogToHeader?

Normally, inspecting access log content requires logging into a server or accessing a log storage system. DebugAccessLogToHeader eliminates that friction: after the request is processed, it serializes the complete access log (identical to what is written to log files) as JSON and injects it into the `X-Debug-Access-Log` response header. You can view the access log directly using `curl -v` or your browser's developer tools.

## Quick Start

### Simplest Configuration

This plugin has no configuration parameters — simply enable it.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: debug-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "dev.example.com"
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
            name: debug-access-log
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-access-log
  namespace: default
spec:
  requestPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

Once enabled, every response will include a header like:

```
X-Debug-Access-Log: {"remote_addr":"192.168.1.1","method":"GET","uri":"/api/users","status":200,"latency_ms":12,...}
```

---

## Configuration Parameters

This plugin has no configuration parameters.

---

## Common Configuration Scenarios

### 1. Enable for All Routes (Development Environment)

Enable the debug header globally on all routes in a development gateway, making it easy to inspect the access log for any request at any time.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: dev-all-debug
  namespace: dev
spec:
  parentRefs:
    - name: dev-gateway
  hostnames:
    - "dev.example.com"
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-log-header
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-log-header
  namespace: dev
spec:
  requestPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

### 2. Target a Specific Path

Inject debug headers only on `/api/v2/` path, leaving other routes unaffected. Useful for debugging specific endpoints in a test environment.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: partial-debug
  namespace: test
spec:
  parentRefs:
    - name: test-gateway
  hostnames:
    - "test.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-v2-only
      backendRefs:
        - name: backend-service
          port: 8080
    - backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-v2-only
  namespace: test
spec:
  requestPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

### 3. Combine with Other Plugins

Combine the debug header with an authentication plugin to inspect the access log for authenticated requests (e.g., verifying that JWT claims are correctly reflected in the access log).

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-with-auth
  namespace: dev
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secret: "dev-secret"
    - type: DebugAccessLogToHeader
      config: {}
```

Plugins execute in order. `DebugAccessLogToHeader` captures the complete access log after authentication has finished (including JWT parse results).

---

## FAQ

### Q1: What information is visible in the `X-Debug-Access-Log` header?

**A**: The same information recorded in the access log file, which typically includes client IP, request method, URI, response status code, latency, upstream address, plugin processing results, and more. The exact fields depend on your access log configuration. See the [Access Log documentation](../../../../ops-guide/observability/access-log.md) for details.

### Q2: The response header content appears to be truncated. What can I do?

**A**: HTTP response headers have length limits (dependent on client and proxy). A long access log may be truncated. If this happens, read the log file directly, or reduce the number of fields in your access log configuration.

### Q3: Can I temporarily enable this in production to troubleshoot an issue?

**A**: Not recommended. Even briefly, this plugin can expose user IPs, authentication details, and other sensitive data to the request sender, violating security best practices. For production troubleshooting, use [Access Log](../../../../ops-guide/observability/access-log.md) or a distributed tracing system instead.

---

## Security Best Practices

### Recommended

- Only create this plugin in development, test, or staging environment namespaces
- Use Kubernetes RBAC to restrict who can create `EdgionPlugins` resources
- Use namespace isolation to prevent test plugins from being inadvertently applied to production routes

### Avoid

- Do not create this plugin in production environments, even temporarily
- Do not deploy `EdgionPlugins` resources containing this plugin to production namespaces
- Do not log the content of `X-Debug-Access-Log` headers to client-accessible logging systems

---

## Related Documentation

- [AllEndpointStatus Plugin](all-endpoint-status.md) — Fan-out health probe across all backend endpoints
- [Access Log Configuration](../../../../ops-guide/observability/access-log.md) — Access log fields and format reference
- [Mock Plugin](mock.md) — Return static responses (useful for debugging)
