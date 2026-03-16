# Kubernetes Deployment Guide

Deploy Edgion API Gateway to a Kubernetes cluster using Gateway API.

## Prerequisites

- Kubernetes 1.26+
- `kubectl` configured with cluster access
- Cluster admin permissions (for CRD and RBAC installation)

## Quick Start

```bash
# Deploy everything (CRDs, controller, gateway, base config)
deploy/kubernetes/scripts/deploy.sh -y
```

This single command will:

1. Create the `edgion-system` namespace
2. Install Gateway API CRDs (experimental channel, v1.4.0) and Edgion CRDs
3. Deploy the controller (2 replicas with leader election)
4. Deploy the gateway (2 replicas with readiness probes)
5. Apply the default GatewayClass, EdgionGatewayConfig, and Gateway listeners (HTTP :80, HTTPS :443)

## Directory Structure

```
deploy/kubernetes/
├── namespace.yaml              # edgion-system namespace
├── versions.env                # Default image versions
├── controller/
│   ├── rbac.yaml               # ServiceAccount, ClusterRole, ClusterRoleBinding
│   ├── configmap.yaml          # Controller TOML configuration
│   ├── deployment.yaml         # Controller deployment (2 replicas)
│   └── service.yaml            # Controller + leader services
├── gateway/
│   ├── configmap.yaml          # Gateway TOML configuration
│   ├── deployment.yaml         # Gateway deployment (2 replicas)
│   └── service.yaml            # Gateway service (80, 443, 8080, 8443, admin, metrics)
├── base-config/
│   ├── 01-GatewayClass.yaml        # GatewayClass "public-gateway"
│   ├── 02-EdgionGatewayConfig.yaml # Default gateway tuning parameters
│   ├── 03-Gateway-80.yaml          # HTTP listener on port 80
│   └── 04-Gateway-443.yaml         # HTTPS listener on port 443 (EdgionTls)
└── scripts/
    ├── deploy.sh               # Full deployment script
    ├── cleanup.sh              # Cleanup script
    └── install_crds.sh         # CRD installation script
```

## Step-by-Step Deployment

If you prefer manual control over each step:

### 1. Create Namespace

```bash
kubectl apply -f deploy/kubernetes/namespace.yaml
```

### 2. Install CRDs

```bash
deploy/kubernetes/scripts/install_crds.sh
```

This installs both the standard [Gateway API CRDs](https://gateway-api.sigs.k8s.io/) (experimental channel for TCP/TLS/UDP route support) and Edgion-specific CRDs (EdgionTls, EdgionPlugins, etc.).

### 3. Deploy Controller

```bash
kubectl apply -f deploy/kubernetes/controller/
```

The controller watches Kubernetes resources and streams configuration to gateways via gRPC. It runs in HA mode with leader election — only the leader pod actively watches and syncs.

### 4. Deploy Gateway

```bash
kubectl apply -f deploy/kubernetes/gateway/
```

The gateway is the stateless data plane. It connects to the controller's leader service for configuration and handles all traffic proxying.

### 5. Apply Base Config

```bash
kubectl apply -f deploy/kubernetes/base-config/
```

This creates:
- **GatewayClass** `public-gateway` — references the Edgion controller
- **EdgionGatewayConfig** — server tuning, real IP, timeouts
- **Gateway** listeners — HTTP on port 80, HTTPS on port 443

## Configuration

### Controller Config

Edit `controller/configmap.yaml` to customize:

| Setting | Default | Description |
|---------|---------|-------------|
| `conf_center.watch_namespaces` | `[]` (all) | Limit namespaces to watch |
| `conf_center.ha_mode` | `leader-only` | HA mode: `leader-only` or `all-serve` |
| `conf_center.leader_election.*` | — | Lease timing parameters |
| `logging.log_level` | `info` | Controller log level |

### Gateway Config

Edit `gateway/configmap.yaml` to customize:

| Setting | Default | Description |
|---------|---------|-------------|
| `server.threads` | CPU cores | Pingora worker threads |
| `server.upstream_keepalive_pool_size` | 128 | Upstream connection pool |
| `access_log.enabled` | `true` | Enable access logging |
| `logging.log_level` | `info` | Gateway log level |

### Resource Limits

Edit `gateway/deployment.yaml` or `controller/deployment.yaml` to adjust CPU/memory:

```yaml
resources:
  requests:
    cpu: 250m
    memory: 256Mi
  limits:
    cpu: 1000m
    memory: 1Gi
```

### Image Versions

Override via environment variables or edit `versions.env`:

```bash
CONTROLLER_IMAGE=myregistry/edgion-controller:latest \
GATEWAY_IMAGE=myregistry/edgion-gateway:latest \
  deploy/kubernetes/scripts/deploy.sh -y
```

## Adding Listeners

The base config includes HTTP :80 and HTTPS :443. To add more listeners, create additional Gateway resources:

```yaml
# Example: HTTP on port 8080
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gateway-8080
  namespace: edgion-system
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http-alt
      protocol: HTTP
      port: 8080
      allowedRoutes:
        namespaces:
          from: All
```

Remember to expose the port in the gateway Service if adding non-default ports.

## Exposing the Gateway

The gateway Service defaults to `ClusterIP`. To expose it externally:

```bash
# NodePort
kubectl patch svc edgion-gateway -n edgion-system -p '{"spec":{"type":"NodePort"}}'

# LoadBalancer
kubectl patch svc edgion-gateway -n edgion-system -p '{"spec":{"type":"LoadBalancer"}}'
```

## Cleanup

```bash
# Remove Edgion (keep CRDs and namespace)
deploy/kubernetes/scripts/cleanup.sh

# Full removal including CRDs and namespace
deploy/kubernetes/scripts/cleanup.sh --with-crds --with-namespace
```

## Troubleshooting

```bash
# Check controller logs
kubectl logs -n edgion-system -l app=edgion-controller -f

# Check gateway logs
kubectl logs -n edgion-system -l app=edgion-gateway -f

# Verify gateway readiness
kubectl get pods -n edgion-system

# Check Gateway API resources
kubectl get gatewayclasses,gateways -n edgion-system
```
