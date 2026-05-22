---
name: deployment-patterns
description: Common Edgion deployment patterns: FileSystem local development, K8s production deployment, HA mode.
---

# Deployment Patterns

## Pattern Overview

| Pattern | conf_center | Use Case | Characteristics |
|---------|-------------|----------|-----------------|
| FileSystem | `type = "filesystem"` | Local development/testing/CI | Driven by YAML files, no K8s required |
| Kubernetes single instance | `type = "kubernetes"` | Small scale/K8s testing | Watches the K8s API directly |
| Kubernetes HA | `type = "kubernetes"` + `ha_mode = true` | Production | Leader election, multi-replica high availability |

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
  type: filesystem
  conf_dir: config/resources     # YAML resource directory
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
edgion-gateway --server-addr http://127.0.0.1:50051
```

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
  gateway_class: edgion          # Required: name of the GatewayClass to match

  # Optional: namespace filter
  watch_namespaces: [default, production]

  # Optional: label filter
  metadata_filter:
    label_selector: app=edgion
```

### Key Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `gateway_class` | `String` | **Required** | Only process resources for this GatewayClass |
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
  gateway_class: edgion
  ha_mode: "true"
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
| `lease_namespace` | `String` | `"edgion-system"` | Lease resource namespace |
| `lease_duration_secs` | `u64` | `15` | Lease duration (validity on the apiserver side) |
| `renew_deadline_secs` | `u64` | `10` | Renewal deadline window (window absorbing transient errors) |
| `retry_period_secs` | `u64` | `2` | Uniform sleep interval between each acquire/renew attempt (corresponds to client-go `RetryPeriod`) |

### HA Behavior
- **Leader only** performs: resource processing, status write-back, ACME certificate issuance, gRPC push.
- **Standby** behavior: watches K8s resources but does not process them, waiting for Leader failover.
- **Failover**: after the Leader lease expires, Standbys compete to become the new Leader and fully reprocess from scratch.

---

## Deployment Topology Recommendations

| Scenario | Controller | Gateway | Recommendation |
|----------|------------|---------|----------------|
| Development | 1 (FileSystem) | 1 | Single process, fast iteration |
| Testing | 1 (K8s) | 1-2 | Validate K8s integration |
| Production | 3 (K8s HA) | 2+ | High availability, scale Gateway as needed |

### Gateway Scaling
- Gateway is a **stateless** data plane and can be horizontally scaled freely.
- Each Gateway Pod fetches its configuration independently from the Controller.
- Traffic is distributed via K8s Service/LoadBalancer.
