# Stream Plugins

> **🔌 Edgion extension**
> 
> Stream plugins are connection-level (L4) plugins provided by the `EdgionStreamPlugins` CRD; they run before HTTP parsing and are not part of standard Gateway API.

Stream plugins apply access control to raw TCP/TLS/UDP connections at Pingora's
ConnectionFilter stage. They decide whether a connection (or, for UDP, a
session) is allowed before any payload is forwarded.

## How attachment works

Stream plugins are **not** inline in the route. You define an
`EdgionStreamPlugins` resource and reference it from the route with the
`edgion.io/edgion-stream-plugins` annotation. The annotation value is
`namespace/name` (or just `name` when the plugin lives in the same namespace as
the route).

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/tcp-ip-restriction-deny"
```

The same annotation works on `TCPRoute`, `TLSRoute`, `UDPRoute`, and at the
Gateway listener level. On `UDPRoute` only the Stage-1 ConnectionFilter
(`spec.plugins`) runs, because UDP has no TLS handshake.

## Supported plugins

| Type | Config object | Description |
|------|---------------|-------------|
| `IpRestriction` | inline rules | IP allow/deny groups embedded directly in the plugin. |
| `GlobalConnectionIpRestriction` | `pluginMetadataRef` | References a `GlobalConnectionIpRestriction` PluginMetaData; Center-managed and synced across Controllers. |

Each entry under `spec.plugins` has an optional `enable` flag (default `true`),
a `type`, and a `config` object.

### `IpRestriction` config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allow` | Array\<Object\> | none | Allow groups; each is `{name, cidrs}`. A non-empty `allow` turns on whitelist mode. |
| `deny` | Array\<Object\> | none | Deny groups; each is `{name, cidrs}`. Evaluated before `allow`. |
| `defaultAction` | Enum | `allow` | Action when no rule matches. Allowed values: `allow` \| `deny`. |

At least one of `allow` or `deny` must be non-empty.

#### Group sub-fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | **Required** | Group name, unique within the plugin (admin label only). |
| `cidrs` | Array\<String\> | **Required** | CIDR strings or bare IPs (bare IPs become `/32` or `/128`). |
| `description` | String | none | Optional human-readable note. |

Evaluation order: `deny` first, then `allow`, then `defaultAction`. For raw TCP
a denied connection is simply closed (no protocol-level response exists at L4).

### `GlobalConnectionIpRestriction` config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `pluginMetadataRef.name` | String | **Required** | Name of the `GlobalConnectionIpRestriction` PluginMetaData. |
| `pluginMetadataRef.namespace` | String | parent ESP namespace | Namespace of the PluginMetaData; inherited from the parent `EdgionStreamPlugins` if omitted. |

The referenced PluginMetaData stores the rules centrally under
`spec.metadata.config`, with a kill switch (`enable`), a selected `activeProfile`,
and named `profiles` so you can swap rule sets instantly. This lets multiple
Controllers share one policy.

## Quick start (inline IpRestriction)

### Step 1: Define the plugin

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tcp-ip-restriction-allow
  namespace: edgion-test
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - name: allowed-clients
            cidrs:
              - "10.0.0.0/8"          # internal network only
        defaultAction: deny           # reject everything not allowed
```

### Step 2: Reference it from the route

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: redis-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/tcp-ip-restriction-allow"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

### Step 3: Apply

```bash
kubectl apply -f stream-plugins.yaml
kubectl apply -f tcp-route.yaml
```

Only clients in `10.0.0.0/8` can now reach the Redis service.

## Center-managed restriction (GlobalConnectionIpRestriction)

Store the rules once in a `GlobalConnectionIpRestriction` PluginMetaData (which
supports named `profiles` and a kill switch) and reference it from a stream
plugin. This is the form Center syncs to multiple Controllers.

```yaml
apiVersion: edgion.io/v1
kind: PluginMetaData
metadata:
  name: gcir-policy
  namespace: edgion-test
spec:
  metadata:
    type: GlobalConnectionIpRestriction
    config:
      enable: true                    # kill switch; false disables the policy
      activeProfile: main             # which profile is live
      profiles:
        main:
          defaultAction: deny
          allow:
            - name: office
              cidrs:
                - "10.0.0.0/8"
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: gcir-stream
  namespace: edgion-test
spec:
  plugins:
    - type: GlobalConnectionIpRestriction
      config:
        pluginMetadataRef:
          name: gcir-policy
          # namespace omitted: inherits parent ESP namespace (edgion-test)
---
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: gcir-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/gcir-stream"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

## Cross-namespace reference

Point at a plugin in another namespace using `namespace/name`:

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: app-route
  namespace: app
  annotations:
    edgion.io/edgion-stream-plugins: "security/common-policy"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

## Edge cases

### Same annotation on a UDPRoute

`UDPRoute` accepts the identical annotation. Only Stage-1 (`spec.plugins`) runs;
the IP check is evaluated on the first datagram of each session (fail-close), so
a denied client never establishes a session.

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: udp-route-with-plugins
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/udp-ip-restriction-deny"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: udp-plugins
  rules:
    - backendRefs:
        - name: udp-echo-backend
          port: 9300
```

### Deny a single host inside an allowed range

`deny` is evaluated before `allow`, so you can permit a broad CIDR while carving
out one address. Group names are arbitrary labels used only for organization.

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: carve-out
  namespace: edgion-test
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - name: internal
            cidrs:
              - "10.0.0.0/8"
        deny:
          - name: quarantine
            cidrs:
              - "10.6.6.6/32"        # blocked even though it is inside 10.0.0.0/8
        defaultAction: deny
```

## Troubleshooting

### Plugin not taking effect

1. Confirm the plugin resource exists:
   ```bash
   kubectl get edgionstreamplugins -A
   ```
2. Check the annotation value (must be `name` or `namespace/name`):
   ```bash
   kubectl get tcproute <name> -o yaml | grep -A2 annotations
   ```
3. Check the Gateway logs:
   ```bash
   kubectl logs <gateway-pod> | grep -i "stream plugin"
   ```

### Connection refused unexpectedly

`deny` wins over `allow`, and `defaultAction: deny` rejects anything not listed.
Verify your client IP falls inside an `allow` group's `cidrs` and is not caught
by a `deny` group.
