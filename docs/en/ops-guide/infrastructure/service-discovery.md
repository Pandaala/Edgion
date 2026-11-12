# Service Discovery Integration

Edgion supports integrating with external service discovery registries (such as HashiCorp Consul, Nacos, Eureka, and Apache ZooKeeper) without modifying the Edgion core. This integration is achieved by leveraging the **Registry Synchronization (Sync Bridge)** pattern.

By separating the service discovery synchronizer from the data plane and control plane, Edgion remains lightweight, high-performance, and completely decoupled from third-party client SDK lifecycles.

---

## Architectural Concept

In both Kubernetes and non-Kubernetes environments, Edgion represents backends using standard Kubernetes-compatible resources: `Service` and `EndpointSlice`. 

External service discovery integration utilizes a **Sync Bridge** (such as an operator, sidecar daemon, or utility script) that subscribes to the source registry, translates service endpoints into Edgion-compatible `EndpointSlice` resources, and writes them to Edgion's configuration center.

```
+-----------------------------------------------------------+
|                   External Registry                       |
|           (Consul / Nacos / Eureka / ZooKeeper)           |
+---------------------+-------------------------------------+
                      | (Watch / Poll)
                      v
+---------------------+-------------------------------------+
|                  Registry Sync Bridge                     |
|    (e.g., consul-k8s, consul-template, or custom sync)    |
+---------------------+-------------------------------------+
                      | (Write)
                      v
+---------------------+-------------------------------------+
|              Edgion Configuration Center                  |
|     (Kubernetes API Server / Etcd Key / File System)      |
+---------------------+-------------------------------------+
                      | (Dynamic Watch)
                      v
+---------------------+-------------------------------------+
|                 Edgion Controller                         |
+---------------------+-------------------------------------+
                      | (gRPC Sync)
                      v
+---------------------+-------------------------------------+
|                  Edgion Gateway                           |
|                  (Data Plane LB)                          |
+-----------------------------------------------------------+
```

---

## Endpoint Readiness

Edgion follows the Kubernetes `discovery.k8s.io/v1` contract for `endpoints[].conditions.ready`:

| `conditions.ready` | Gateway behavior |
|:---|:---|
| `true` | The endpoint receives traffic. |
| Omitted (no `conditions`, empty `conditions`, or no `ready` key) | Treated as ready and receives traffic, the same as kube-proxy. |
| `false` | The endpoint is excluded from load balancing. |

A sync bridge therefore does not need to write `conditions` for healthy instances. To take an instance out of rotation, remove it from the slice or set `ready: false`:

```yaml
apiVersion: discovery.k8s.io/v1
kind: EndpointSlice
metadata:
  name: my-service-slice
  namespace: default
  labels:
    kubernetes.io/service-name: my-service
addressType: IPv4
ports:
  - name: http
    port: 8080
endpoints:
  # No conditions: treated as ready
  - addresses:
      - "10.0.0.11"
  # Explicitly not ready: excluded from load balancing
  - addresses:
      - "10.0.0.12"
    conditions:
      ready: false
```

Recommended mapping from registry instance state:

| Registry | Instance state | EndpointSlice endpoint |
|:---|:---|:---|
| Consul | All health checks `passing` or `warning` (Consul DNS also includes `warning` by default) | Ready (omit `conditions` or set `ready: true`) |
| Consul | Any check `critical`, or maintenance mode | `ready: false`, or omit the endpoint |
| Nacos | `healthy: true` and `enabled: true` | Ready |
| Nacos | `healthy: false` or `enabled: false` | `ready: false`, or omit the endpoint |
| Eureka | `UP` | Ready |
| Eureka | `DOWN`, `OUT_OF_SERVICE`, `STARTING`, or `UNKNOWN` | `ready: false`, or omit the endpoint |

> `ready` is a snapshot taken when the sync bridge writes the slice; nothing refreshes it between syncs. To remove a failed instance promptly, attach an `EdgionBackendTrafficPolicy` with [active health checks](../../user-guide/http-route/backends/health-check.md) or [outlier detection](../../user-guide/http-route/backends/circuit-breaker.md) to the synced Service.
>
> The Gateway currently selects endpoints by `ready` only; the `serving` and `terminating` conditions are not used.

---

## 1. HashiCorp Consul Integration

### Kubernetes Environment (consul-k8s Catalog Sync)

In a Kubernetes environment, you can use the official HashiCorp `consul-k8s` tool's **Catalog Sync** feature to synchronize Consul services directly into Kubernetes.

1. **Deploy Catalog Sync**: Configure the Consul Helm chart to enable Catalog Sync:
   ```yaml
   syncCatalog:
     enabled: true
     toK8s: true # Sync Consul services to K8s
   ```
2. **Reconciliation**: `consul-k8s` automatically queries the Consul service catalog and creates corresponding `Service` and `Endpoints` resources inside your Kubernetes namespace.
3. **EndpointSlice Generation**: The Kubernetes built-in `endpointslice-controller` automatically converts the `Endpoints` resources into `EndpointSlice` resources.
4. **Edgion Processing**: Edgion Controller natively watches the generated `EndpointSlice` resources, compiles them, and pushes them to `edgion-gateway` for dynamic load balancing.

### Non-Kubernetes Environment (consul-template)

In a pure VM or bare-metal environment using the `file_system` or `etcd` configuration center, you can use `consul-template` to dynamically write configuration files.

1. **Write Template**: Create a template named `endpoint_slice.yaml.tpl`:
   ```yaml
   apiVersion: discovery.k8s.io/v1
   kind: EndpointSlice
   metadata:
     name: my-service-slice
     namespace: default
   addressType: IPv4
   ports:
     - name: http
       port: 8080
   endpoints:
     {{range service "my-service"}}
     - addresses:
         - "{{.Address}}"
       conditions:
         ready: true
     {{end}}
   ```
2. **Configure consul-template**: Run `consul-template` as a systemd service, watching your template and writing directly into Edgion's file system watch directory:
   ```bash
   consul-template \
     -template "/etc/edgion/templates/endpoint_slice.yaml.tpl:/etc/edgion/conf/EndpointSlice/default/my-service-slice.yaml" \
     -once # omit -once for daemon mode
   ```
3. **Hot Reload**: When instances change in Consul, `consul-template` updates the YAML file. Edgion's file watcher instantly detects the update and hot-reloads the gateway routing backends with zero downtime.

---

## 2. Alibaba Nacos Integration

### Kubernetes Environment (nacos-controller / nacos-istio)

To integrate Nacos with Kubernetes, you can use standard synchronizers like the community-maintained `nacos-controller` or the Nacos-to-K8s synchronizer:

1. **Deployment**: Deploy the sync controller in your Kubernetes cluster.
2. **Synchronization**: The sync controller polls the Nacos registry APIs and creates standard Kubernetes `Service` and `Endpoints` resources.
3. **Consumption**: Edgion Controller reads the automatically generated `EndpointSlice` resources from the Kubernetes API.

### Non-Kubernetes Environment (nacos-istio / Custom Daemon)

For VM-based setups, you can write a lightweight cron job or a daemon script (Go/Python) that calls the Nacos HTTP API and writes configuration files.

1. **Query Nacos API**: Poll `/nacos/v1/ns/instance/list?serviceName=my-service`.
2. **Generate YAML**: Extract the `ip` and `port` fields, and write them into the Edgion format `EndpointSlice` YAML.
3. **Reconcile**: Save the file to `/etc/edgion/conf/EndpointSlice/default/my-service.yaml` or PUT it to the Edgion Controller Admin API:
   ```bash
   curl -X PUT \
     -H "Authorization: Bearer <your-admin-token>" \
     -H "Content-Type: application/yaml" \
     --data-binary @my-service-slice.yaml \
     http://127.0.0.1:12101/api/v1/namespaced/EndpointSlice/default/my-service-slice
   ```

---

## 3. Spring Cloud Eureka Integration

Spring Cloud Eureka acts as a REST-based service registry. Because it does not run sidecar agents, integration is done using a polling/sync daemon.

### Kubernetes Environment (Spring Cloud Kubernetes)

When migrating Spring Cloud workloads to Kubernetes:
1. Use **Spring Cloud Kubernetes Discovery Server** to sync Eureka registrations to native Kubernetes services.
2. Once synced, Edgion automatically consumes them via standard `EndpointSlice` resources.

### Non-Kubernetes Environment (Eureka HTTP Sync Daemon)

You can deploy a small sync daemon next to Edgion Controller:
1. **Poll Eureka**: Query the Eureka REST API `GET /eureka/apps/MY-SERVICE`.
2. **Format Conversion**: Convert the returned XML/JSON application instance IPs and ports into an Edgion `EndpointSlice` structure.
3. **Sync to Edgion**: Write to the file system configuration directory or send to the Edgion Controller Admin API via HTTP.

---

## 4. Apache ZooKeeper Integration (Dubbo / Legacy Java RPC)

ZooKeeper uses hierarchical ZNodes to store provider addresses (e.g., `/dubbo/com.foo.BarService/providers`).

### Sync Daemon Pattern

Since ZooKeeper is a hierarchical registry rather than a REST service, a persistent watcher daemon is recommended:

1. **Watch ZooKeeper Path**: The sync daemon connects to ZooKeeper and subscribes to watches on the `/dubbo/{service_name}/providers` node.
2. **Extract Endpoints**: When Dubbo providers go online or offline, ZooKeeper triggers a watch callback. The daemon extracts the IP addresses and ports from the provider URI strings.
3. **Publish to Edgion**: The daemon compiles the list and updates the corresponding Edgion `EndpointSlice` resource via the Edgion Controller Admin API or direct file updates.

---

## Summary of Integration Strategies

| Environment | Registry | Sync Tool | Output Resource |
|:---|:---|:---|:---|
| **Kubernetes** | Consul | `consul-k8s` catalog sync | Native `Service` & `EndpointSlice` |
| **Kubernetes** | Nacos / Eureka | Sync Operator / Controller | Native `Service` & `EndpointSlice` |
| **VM / Bare-metal** | Consul | `consul-template` | File / Etcd `EndpointSlice` YAML |
| **VM / Bare-metal** | Nacos / Eureka / ZK | Custom sync daemon / script | File / Etcd `EndpointSlice` YAML (via Admin API) |

By leveraging these sync patterns, Edgion maintains a clean code boundary while fully supporting legacy and external service discovery backends with zero performance impact.
