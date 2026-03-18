# Stream Plugins User Guide

> **🔌 Edgion Extension**
> 
> `EdgionStreamPlugins` is an Edgion custom CRD that provides access control for stream-style connections.

Get started with Edgion's stream plugin functionality.

## What are Stream Plugins?

Stream Plugins provide access control and security policies for stream-style connections, such as IP restriction and early connection rejection.

**Supported Protocols**:
- Gateway listener-level connection filtering
- TCPRoute
- TLSRoute

**Currently Supported Plugins**:
- **IP Restriction** - Access control based on client IP address

---

## Quick Start

### Step 1: Create Plugin Configuration

Create an `EdgionStreamPlugins` resource:

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: ip-filter
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        ipSource: remoteAddr      # Use actual connection IP
        allow:                     # Allowlist
          - "10.0.0.0/8"
          - "192.168.1.0/24"
        deny:                      # Denylist (higher priority)
          - "10.0.0.100"
        defaultAction: deny        # Default deny
        message: "Access denied"
```

### Step 2: Reference in Route

Reference the plugin in the TCPRoute `annotations`:

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: my-tcp-route
  namespace: default
  annotations:
    edgion.io/edgion-stream-plugins: ip-filter  # Reference plugin name
spec:
  parentRefs:
    - name: my-gateway
      sectionName: tcp-6379
  rules:
    - backendRefs:
        - name: redis-service
          port: 6379
```

### Step 3: Apply Configuration

```bash
kubectl apply -f stream-plugins.yaml
kubectl apply -f tcp-route.yaml
```

Done! Now only IPs in the allowlist can access your TCP service.

---

## IP Restriction Configuration Details

### Basic Configuration Fields

| Field | Type | Description | Required |
|-------|------|-------------|----------|
| `ipSource` | string | IP source: `remoteAddr` (connection IP) | Yes |
| `allow` | []string | IP allowlist (CIDR format) | No |
| `deny` | []string | IP denylist (higher priority than allowlist) | No |
| `defaultAction` | string | Default action: `allow` or `deny` | Yes |
| `message` | string | Message when access is denied | No |

### Evaluation Logic

```
1. Check deny list → deny if matched
2. Check allow list → allow if matched
3. Apply defaultAction
```

---

## Use Cases

### Scenario 1: Database Access Control

Only allow internal IPs to access Redis:

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: redis-security
spec:
  plugins:
    - type: IpRestriction
      config:
        ipSource: remoteAddr
        allow:
          - "10.0.0.0/8"      # Internal network
        defaultAction: deny
```

### Scenario 2: Block Specific IPs

Allow everyone access but block malicious IPs:

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: block-bad-ips
spec:
  plugins:
    - type: IpRestriction
      config:
        ipSource: remoteAddr
        deny:
          - "1.2.3.4"
          - "5.6.7.0/24"
        defaultAction: allow
```

### Scenario 3: Shared Policy Across Multiple Routes

A single plugin configuration reused by multiple routes:

```yaml
# Define once
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: common-policy
spec:
  plugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
        defaultAction: deny

---
# TCP route uses it
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: common-policy
# ...

---
# TLSRoute can also use it
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TLSRoute
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: common-policy
# ...
```

---

## Cross-Namespace References

Plugins and routes can be in different namespaces using the `namespace/name` format:

```yaml
# Plugin in the security namespace
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: global-policy
  namespace: security
spec:
  plugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
        defaultAction: deny

---
# Route in the app namespace, cross-namespace reference
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: app-route
  namespace: app
  annotations:
    edgion.io/edgion-stream-plugins: security/global-policy  # Cross-namespace
spec:
  # ...
```

---

## Troubleshooting

### Plugin Not Taking Effect

**Checklist**:

1. Confirm the plugin resource exists:
   ```bash
   kubectl get edgionstreamplugins -A
   ```

2. Check if the annotation is correct:
   ```bash
   kubectl get tcproute <name> -o yaml | grep annotations -A 2
   ```
   The current primary key is `edgion.io/edgion-stream-plugins`.

3. Check the Gateway logs:
   ```bash
   kubectl logs <gateway-pod> | grep -i "stream plugin"
   ```

4. Verify namespace matching:
   - Same namespace: use the plugin name directly
   - Cross-namespace: use `namespace/name` format

### Connection Refused

Check if the IP is in the allowlist:

```bash
# Check your IP
curl ifconfig.me

# Check plugin configuration
kubectl get edgionstreamplugins <name> -o yaml
```

---

## Performance Considerations

- IP checks are performed at connection establishment and do not affect data transfer performance
- Uses CIDR matching algorithms for fast lookups
- Plugin configurations are hot-reloaded without Gateway restarts

---

## Next Steps

- [Full Annotations Reference](../../dev-guide/annotations-guide.md)
- [Adding Custom Plugins](../../dev-guide/add-new-resource-guide.md)
- [Architecture Overview](../../dev-guide/architecture-overview.md)

---

**Version**: Edgion v0.1.0
**Last Updated**: 2025-12-25
