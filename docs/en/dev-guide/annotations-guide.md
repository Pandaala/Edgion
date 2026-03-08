# Annotations Usage Guide

This document introduces the design and usage of referencing Stream Plugins through Annotations in Edgion.

## Background

The Kubernetes Gateway API TCPRoute and UDPRoute specifications do not include `filters` or `extensionRef` fields. To extend functionality without violating the Gateway API specification, Edgion uses the Kubernetes Annotations mechanism to implement Stream Plugin references.

## Design Principles

### 1. Gateway API Specification Compliance

- Does not modify standard Gateway API fields
- Uses native Kubernetes Annotations mechanism
- Maintains compatibility with standard Gateway API resources

### 2. Simplicity and Ease of Use

- Reference plugins through a single annotation key
- Support same-namespace and cross-namespace references
- Intuitive configuration, easy to understand

### 3. Flexibility

- Plugins and routes are decoupled for reusability
- Support dynamic updates (hot reload)
- Facilitate centralized security policy management

---

## Gateway Annotations

The following annotations are used to configure Gateway-level behavior:

| Annotation | Type | Default | Description |
|------------|------|---------|-------------|
| `edgion.com/enable-http2` | string | `"true"` | Control HTTP/2 support (h2c and ALPN) |
| `edgion.io/backend-protocol` | string | - | Backend protocol for TLS listeners (set to `"tcp"` to enable TLS termination to TCP) |
| `edgion.io/http-to-https-redirect` | string | `"false"` | Set to `"true"` to enable HTTP to HTTPS redirect |
| `edgion.io/https-redirect-port` | string | `"443"` | HTTPS redirect target port |

For detailed usage, see the [HTTP to HTTPS Redirect Guide](../ops-guide/gateway/http-to-https-redirect.md).

---

## Route Annotations

### Stream Plugins Annotation

**Key**: `edgion.io/stream-plugins`

**Value format**:
- Same-namespace reference: `<plugin-name>`
- Cross-namespace reference: `<namespace>/<plugin-name>`

**Applicable resources**:
- `TCPRoute` (Gateway API v1alpha2)
- `UDPRoute` (Gateway API v1alpha2)

---

## Usage Examples

### 1. Same-Namespace Reference

The most common scenario, where the plugin and route are in the same namespace:

```yaml
# EdgionStreamPlugins definition
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: redis-ip-filter
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - "10.0.0.0/8"
        defaultAction: deny

---
# TCPRoute referencing the plugin
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: redis-route
  namespace: default
  annotations:
    edgion.io/stream-plugins: redis-ip-filter  # Use plugin name directly
spec:
  parentRefs:
    - name: example-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-service
          port: 6379
```

### 2. Cross-Namespace Reference

The security team manages plugins in a dedicated namespace, and business teams reference them cross-namespace:

```yaml
# Plugin in security-policies namespace
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: strict-ip-filter
  namespace: security-policies
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - "10.0.0.0/8"
        defaultAction: deny
        message: "Only internal network access allowed"

---
# Application in app-production namespace references it
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: app-tcp-route
  namespace: app-production
  annotations:
    edgion.io/stream-plugins: security-policies/strict-ip-filter  # Cross-namespace
spec:
  parentRefs:
    - name: prod-gateway
  rules:
    - backendRefs:
        - name: app-backend
          port: 8080
```

### 3. Multiple Routes Sharing the Same Plugin

```yaml
# Define plugin once
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: common-security
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8", "172.16.0.0/12"]
        defaultAction: deny

---
# Multiple TCPRoutes share it
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: service-a-route
  annotations:
    edgion.io/stream-plugins: common-security
spec:
  # ...

---
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: service-b-route
  annotations:
    edgion.io/stream-plugins: common-security
spec:
  # ...

---
# UDPRoute can also use it
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: udp-service-route
  annotations:
    edgion.io/stream-plugins: common-security
spec:
  # ...
```

### 4. Not Using Plugins

If plugins are not needed, simply omit the annotation:

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: public-tcp-route
  namespace: default
  # No edgion.io/stream-plugins annotation
spec:
  parentRefs:
    - name: public-gateway
  rules:
    - backendRefs:
        - name: public-service
          port: 80
```

---

## Implementation Details

### Processing Flow

```
1. ConfigManager loads TCPRoute/UDPRoute
   |
2. Check metadata.annotations["edgion.io/stream-plugins"]
   |
3. Parse reference format (namespace/name or name)
   |
4. Get corresponding EdgionStreamPlugins from StreamPluginStore
   |
5. Inject stream_plugin_runtime into each Rule of the Route
   |
6. Execute stream_plugin_runtime.run() when a connection is established
```

### Code Locations

**Parsing logic**:
- `src/core/routes/tcp_routes/conf_handler_impl.rs`
- `src/core/routes/udp_routes/conf_handler_impl.rs`

**Key method**:

```rust
impl TCPRoute {
    pub fn init_stream_plugins(&mut self, plugin_store: &StreamPluginStore) {
        if let Some(annotations) = &self.metadata.annotations {
            if let Some(plugin_ref) = annotations.get("edgion.io/stream-plugins") {
                // Parse namespace/name or name
                let (plugin_namespace, plugin_name) = parse_plugin_ref(plugin_ref, route_ns);
                
                // Get plugin from store
                if let Some(plugins) = plugin_store.get_by_ns_name(plugin_namespace, plugin_name) {
                    for rule in self.spec.rules.iter_mut() {
                        rule.stream_plugin_runtime = plugins.spec.stream_plugin_runtime.clone();
                    }
                }
            }
        }
    }
}
```

**Execution logic** (TCP example):

```rust
// src/core/routes/tcp_routes/edgion_tcp.rs
async fn handle_connection(&self, downstream: Stream, ctx: &mut TcpContext) {
    // 1. Match TCPRoute
    let tcp_route = match_tcp_route(ctx);
    
    // 2. Get first rule (usually only one)
    if let Some(rule) = tcp_route.spec.rules.first() {
        // 3. Check for stream plugins
        if !rule.stream_plugin_runtime.is_empty() {
            let client_ip = extract_client_ip(&downstream);
            let stream_ctx = StreamContext {
                client_ip,
                listener_port: self.listener_port,
            };
            
            // 4. Execute plugin chain
            match rule.stream_plugin_runtime.run(&stream_ctx).await {
                StreamPluginResult::Allow => {
                    // Continue processing connection
                }
                StreamPluginResult::Deny(reason) => {
                    tracing::info!("Connection denied by plugin: {}", reason);
                    return; // Reject connection
                }
            }
        }
    }
    
    // 5. Select backend and establish connection
    let backend = rule.backend_finder.select(ctx);
    proxy_to_backend(downstream, backend).await;
}
```

---

## Configuration Management

### Hot Reload

Plugin configurations support hot reload without restarting the Gateway:

1. Modify the `EdgionStreamPlugins` resource
2. ConfigServer detects the change
3. Update StreamPluginStore
4. New connections immediately use the new configuration
5. Established connections are not affected (TCP/UDP long-connection characteristic)

### Plugin Update Example

```bash
# Modify plugin configuration
kubectl edit edgionstreamplugins redis-ip-filter

# Or apply new configuration
kubectl apply -f updated-plugin.yaml

# New connections take effect immediately, no Gateway restart needed
```

---

## Best Practices

### 1. Naming Conventions

**Plugin naming**:
- Descriptive names: `<service>-<policy-type>`
- Examples: `redis-ip-filter`, `mysql-rate-limit`, `strict-security`

**Annotation values**:
- Same namespace: Use plugin name directly
- Cross-namespace: Use `namespace/name` format

### 2. Plugin Organization

**Categorize by purpose**:

```
security-policies namespace:
  - strict-ip-filter         # Strict IP restriction
  - public-rate-limit        # Public service rate limiting
  - internal-only            # Internal network only

default namespace:
  - dev-ip-filter            # Development environment
  - staging-ip-filter        # Staging environment
```

### 3. Access Control

Use Kubernetes RBAC to control plugin resource access:

```yaml
# Example: Allow app-team to use but not modify plugins in security-policies
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: plugin-reader
  namespace: security-policies
rules:
  - apiGroups: ["edgion.io"]
    resources: ["edgionstreamplugins"]
    verbs: ["get", "list", "watch"]  # Read-only permissions
```

### 4. Documentation and Comments

Add detailed descriptions to plugin resources:

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: production-security
  namespace: security-policies
  annotations:
    description: "Production environment standard security policy"
    owner: "security-team@company.com"
    last-review: "2025-12-25"
spec:
  plugins:
    - type: IpRestriction
      config:
        # Configuration details...
```

---

## FAQ

### Plugin Not Taking Effect

**Issue**: Annotation is configured but the plugin is not executing

**Troubleshooting steps**:

1. **Check if the annotation key is correct**:
   ```bash
   kubectl get tcproute <name> -o yaml | grep annotations -A 2
   ```
   Confirm it is `edgion.io/stream-plugins` (note spelling and domain)

2. **Check if the plugin resource exists**:
   ```bash
   # Same namespace
   kubectl get edgionstreamplugins <plugin-name> -n <route-namespace>
   
   # Cross-namespace
   kubectl get edgionstreamplugins <plugin-name> -n <plugin-namespace>
   ```

3. **Check Gateway logs**:
   ```bash
   kubectl logs <gateway-pod> | grep -i "stream plugin"
   ```
   
   Possible log messages:
   - `"EdgionStreamPlugins not found: <name>"` - Plugin does not exist
   - `"Loading stream plugins for TCPRoute <name>"` - Normal loading
   - `"Connection denied by plugin: ..."` - Plugin executed and denied connection

### Namespace Issues

**Issue**: Cross-namespace reference fails

**Solution**:
- Confirm format is correct: `namespace/name`
- Check that the plugin resource actually exists in the target namespace

**Incorrect example**:
```yaml
annotations:
  edgion.io/stream-plugins: my-plugin  # Wrong if plugin is in another namespace
```

**Correct example**:
```yaml
annotations:
  edgion.io/stream-plugins: security-policies/my-plugin  # Explicitly specify namespace
```

### Plugin Updates Not Taking Effect

**Issue**: Modified EdgionStreamPlugins, but route behavior hasn't changed

**Causes**:
- Established TCP/UDP connections use the old configuration (long-connection characteristic)
- ConfigServer may not have synced the update yet

**Solutions**:
1. Wait for new connections: Newly established connections will use the new configuration
2. Check ConfigServer sync status
3. Confirm the plugin resource's `resourceVersion` has been updated

---

## Comparison with HTTPRoute

### HTTPRoute Uses `filters`

The HTTPRoute specification includes a `filters` field for direct plugin references:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-http-plugin
```

### TCPRoute/UDPRoute Uses Annotations

Since the TCPRoute/UDPRoute specification does not include `filters`, annotations are used:

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  annotations:
    edgion.io/stream-plugins: my-stream-plugin
spec:
  rules:
    - backendRefs: [...]
```

**Comparison summary**:

| Feature | HTTPRoute | TCPRoute/UDPRoute |
|---------|-----------|-------------------|
| Reference method | `spec.rules.filters` | `metadata.annotations` |
| Specification support | Gateway API standard | Requires extension mechanism |
| Implementation | ExtensionRef | Annotations |
| Flexibility | Different filters per rule | Shared plugin for entire Route |
| Granularity | Fine-grained (per-rule) | Coarse-grained (per-route) |

---

## Future Plans

### 1. Multiple Plugin References

Currently a Route can only reference one EdgionStreamPlugins resource. Future support may include:

```yaml
annotations:
  edgion.io/stream-plugins: "ip-filter,rate-limit,audit-log"
```

### 2. Per-Rule Plugin Configuration

Exploring finer-grained plugin configuration:

```yaml
annotations:
  edgion.io/stream-plugins.rule-0: "strict-filter"
  edgion.io/stream-plugins.rule-1: "loose-filter"
```

### 3. Plugin Priority

Support execution order control for multiple plugins:

```yaml
spec:
  plugins:
    - type: IpRestriction
      priority: 100  # Execute first
    - type: RateLimit
      priority: 50   # Execute second
```

---

## References

- [Kubernetes Gateway API - TCPRoute](https://gateway-api.sigs.k8s.io/api-types/tcproute/)
- [Kubernetes Gateway API - UDPRoute](https://gateway-api.sigs.k8s.io/api-types/udproute/)
- [Kubernetes Annotations](https://kubernetes.io/docs/concepts/overview/working-with-objects/annotations/)
- [Edgion Architecture Overview](./architecture-overview.md)
- [Stream Plugins User Guide](../user-guide/tcp-route/stream-plugins.md)

---

**Version**: Edgion v0.1.0  
**Last updated**: 2025-12-25
