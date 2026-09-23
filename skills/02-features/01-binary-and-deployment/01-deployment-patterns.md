---
name: deployment-patterns
description: Common Edgion deployment patterns: FileSystem, Etcd, Kubernetes, and Kubernetes HA.
---

# Deployment Patterns

## Pattern Overview

| Pattern | conf_center | Use Case | Characteristics |
|---------|-------------|----------|-----------------|
| FileSystem | `type: file_system` | Local development/testing/CI | Driven by YAML files, no K8s required |
| Etcd | `type: etcd` | Non-Kubernetes persistent control plane | Watches a prefixed etcd key space |
| Kubernetes single instance | `type = "kubernetes"` | Small scale/K8s testing | Watches the K8s API directly |
| Kubernetes HA | `type: kubernetes` + `ha_mode: leader-only` or `all-serve` | Production | Lease election is always enabled |

## FileSystem Mode

```
                edgion-controller              edgion-gateway
                (FileSystem ConfCenter)        (data plane)
  YAML Files ──► conf_dir ──► process ──► gRPC ──► proxy traffic
```

### Configuration Example

```yaml
# edgion-controller.yaml
conf_center:
  type: file_system
  controller_name: edgion.io/gateway-controller
  conf_dir: config/resources     # YAML resource directory

# Required explicit transport choice for local plaintext development only.
conf_sync_security:
  tls:
    skip_tls: true
```

### Use Cases
- Local development and debugging
- CI/CD integration testing
- Standalone deployments that do not depend on K8s

### Typical Startup

```bash
# Terminal 1: Controller
edgion-controller --conf-dir config/resources

# Terminal 2: Gateway
edgion-gateway --config-file config/dev-gateway.yaml --server-addr http://127.0.0.1:12151
```

`config/dev-gateway.yaml` must also contain the explicit insecure choice
`conf_sync_security: { tls: { skip_tls: true } }`. Prefer matching mTLS certificate
paths outside isolated local development.

---

## Etcd Mode

```yaml
conf_center:
  type: etcd
  controller_name: edgion.io/gateway-controller
  endpoints: ["https://etcd-0:2379", "https://etcd-1:2379"]
  key_prefix: /edgion
  connect_timeout_ms: 5000
  request_timeout_ms: 10000
  tls:
    ca_cert_file: /etc/edgion/etcd/ca.pem
    client_cert_file: /etc/edgion/etcd/client.pem
    client_key_file: /etc/edgion/etcd/client-key.pem
  # auth:                              # Optional; keep credentials in a Secret-mounted file
  #   username: edgion
  #   password: "..."
```

Etcd uses the same Controller processing pipeline as FileSystem mode and persists
resource status under the configured prefix. Configure `conf_sync_security` separately;
it protects Controller-to-Gateway traffic, not Controller-to-etcd traffic.

---

## Kubernetes Mode

```
  K8s API Server
       │ watch/list
       ▼
  edgion-controller (KubernetesCenter)
       │ gRPC
       ▼
  edgion-gateway × N (data plane Pods)
```

### Configuration Example

```yaml
# edgion-controller-k8s.yaml
conf_center:
  type: kubernetes

  # Optional: namespace filter
  watch_namespaces: [default, production]

  # Optional: label filter
  label_selector: app=edgion
```

### Key Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `watch_namespaces` | `Vec<String>` | All | List of namespaces to restrict watching to |
| `gateway_address` | `String?` | — | Address reported in the Gateway status |

---

## Kubernetes HA Mode

```
  edgion-controller-0 (Leader)  ◄── Writes status, pushes via gRPC
  edgion-controller-1 (Standby) ◄── Waits for the Leader lease to expire
  edgion-controller-2 (Standby)
       │ gRPC (Leader only)
       ▼
  edgion-gateway × N
```

### Configuration Example

```yaml
conf_center:
  type: kubernetes
  ha_mode: leader-only
  leader_election:
    lease_name: edgion-controller-leader
    lease_namespace: edgion-system
    lease_duration_secs: 15
    renew_deadline_secs: 10
    retry_period_secs: 2
```

### Leader Election Field Schema

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `lease_name` | `String` | `"edgion-controller-leader"` | Lease resource name |
| `lease_namespace` | `String` | `POD_NAMESPACE`, else `"default"` | Lease resource namespace |
| `lease_duration_secs` | `u64` | `15` | Lease duration (validity on the apiserver side) |
| `renew_deadline_secs` | `u64` | `10` | Renewal deadline window (window absorbing transient errors) |
| `retry_period_secs` | `u64` | `2` | Uniform sleep interval between each acquire/renew attempt (corresponds to client-go `RetryPeriod`) |

### HA Behavior
- `leader-only` (default): only the leader runs watchers, processing, and gRPC; followers wait for the lease and do not watch resources.
- `all-serve`: every replica watches, processes, and serves gRPC; only the leader performs coordinated writes such as status and ACME work.
- After lease loss, replicas compete according to the configured retry/deadline windows. A newly active leader rebuilds state through the normal list/watch lifecycle.

---

## Deployment Topology Recommendations

| Scenario | Controller | Gateway | Recommendation |
|----------|------------|---------|----------------|
| Development | 1 (FileSystem) | 1 | Single process, fast iteration |
| Testing | 1 (K8s) | 1-2 | Validate K8s integration |
| Production | 3 (K8s HA) | 2+ | Start with `leader-only`; use `all-serve` when every Controller replica must serve conf_sync |

### Gateway Scaling
- Gateway is a **stateless** data plane and can be horizontally scaled freely.
- Each Gateway Pod fetches its configuration independently from the Controller.
- Traffic is distributed via K8s Service/LoadBalancer.
