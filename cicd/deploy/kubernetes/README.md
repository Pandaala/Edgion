# Kubernetes Deployment Guide

Deploy Edgion API Gateway to a Kubernetes cluster using Gateway API.

## Prerequisites

- Kubernetes 1.26+
- `kubectl` configured with cluster access
- Cluster admin permissions (for CRD and RBAC installation)

## Quick Start

```bash
# Deploy everything (CRDs, controller, gateway, base config)
cicd/deploy/kubernetes/scripts/deploy.sh -y
```

This single command will:

1. Create the `edgion-system` namespace
2. Install Gateway API CRDs (experimental channel, v1.5.0 by default) and Edgion CRDs
3. Deploy the controller (2 replicas with leader election)
4. Deploy the gateway (2 replicas with readiness probes)
5. Apply the default GatewayClass, EdgionGatewayConfig, and Gateway listeners (HTTP :80, HTTPS :443)

## Directory Structure

```
cicd/deploy/kubernetes/
├── namespace.yaml              # edgion-system namespace
├── versions.env                # Default image versions
├── controller/
│   ├── rbac.yaml               # ServiceAccount, ClusterRole, ClusterRoleBinding
│   ├── configmap.yaml          # Controller YAML configuration
│   ├── deployment.yaml         # Controller deployment (2 replicas)
│   ├── service.yaml            # Controller + leader services
│   └── optional/
│       └── networkpolicy.yaml  # Optional: restrict admin/gRPC ingress (NOT auto-applied)
├── gateway/
│   ├── configmap.yaml          # Gateway YAML configuration
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
kubectl apply -f cicd/deploy/kubernetes/namespace.yaml
```

### 2. Install CRDs

```bash
cicd/deploy/kubernetes/scripts/install_crds.sh
```

This installs the [Gateway API CRDs](https://gateway-api.sigs.k8s.io/) from the experimental channel (needed for TCPRoute and UDPRoute, while also serving the Standard resources) and Edgion-specific CRDs (EdgionTls, EdgionPlugins, etc.).

### 3. Deploy Controller

```bash
kubectl apply -f cicd/deploy/kubernetes/controller/
```

The controller watches Kubernetes resources and streams configuration to gateways via gRPC. It runs in HA mode with leader election — only the leader pod actively watches and syncs.

### 4. Deploy Gateway

```bash
kubectl apply -f cicd/deploy/kubernetes/gateway/
```

The gateway is the stateless data plane. It connects to the controller's leader service for configuration and handles all traffic proxying.

### 5. Apply Base Config

```bash
kubectl apply -f cicd/deploy/kubernetes/base-config/
```

This creates:
- **GatewayClass** `public-gateway` — references the Edgion controller
- **EdgionGatewayConfig** — server tuning, real IP, timeouts
- **Gateway** listeners — HTTP on port 80, HTTPS on port 443

## Admin Credentials

Admin API auth is a single pre-shared bearer token. The controller reads it from the
`edgion-admin-credentials` Secret via the env var `EDGION_ADMIN_TOKEN` (injected via
`secretKeyRef` in `controller/deployment.yaml`), or from the `admin_token` field in a
Secret-mounted config file. There is no username/password and no OIDC for the Admin API.
No token is auto-generated: a blank token means the server returns 503 fail-close on all
Admin API business routes at startup.

### How `deploy.sh` bootstraps the Secret

`deploy.sh` creates the Secret **before** applying the controller manifests, using the
operator's own kubectl (not the controller ServiceAccount). The step is idempotent: if the
Secret already exists, it is left untouched so the current token is preserved. This means
re-running `deploy.sh` on an existing cluster never resets the token.

```bash
kubectl -n edgion-system get secret edgion-admin-credentials >/dev/null 2>&1 || \
  kubectl -n edgion-system create secret generic edgion-admin-credentials \
    --from-literal=token="$(openssl rand -hex 32)"
```

### Viewing the token

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

### Rotating the token

Edit the Secret directly; the controller picks up the new value on restart (or via a
rolling update). All previously issued requests using the old token are immediately
rejected once the controller reloads.

```bash
kubectl -n edgion-system patch secret edgion-admin-credentials \
  -p "{\"stringData\":{\"token\":\"$(openssl rand -hex 32)\"}}"
kubectl rollout restart deployment/edgion-controller -n edgion-system
```

### CLI and API authentication

- **CLI**: `edgion-cli login --token <TOKEN>` — persists the token to `~/.edgion/token`.
- **API**: `Authorization: Bearer <TOKEN>` header on all Admin API requests.

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
  cicd/deploy/kubernetes/scripts/deploy.sh -y
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

> **Note**: changing the gateway Service to `NodePort` or `LoadBalancer` publishes all ports declared in the Service to external traffic — including the metrics port (12090). The admin port (12001) and the probe port (12000) are intentionally **not** included in the gateway Service and are therefore never externally published, regardless of Service type.

## Cleanup

```bash
# Remove Edgion (keep CRDs and namespace)
cicd/deploy/kubernetes/scripts/cleanup.sh

# Full removal including CRDs and namespace
cicd/deploy/kubernetes/scripts/cleanup.sh --with-crds --with-namespace
```

## Optional Hardening

### Admin NetworkPolicy

`controller/optional/networkpolicy.yaml` restricts ingress to the controller pod to only
the admin API port (12101) and conf_sync gRPC port (12151). It is **not** applied
automatically by `deploy.sh` — `kubectl apply -f <dir>` is non-recursive, so files in
subdirectories are never auto-picked-up.

To apply it manually after editing the selectors for your environment:

```bash
kubectl apply -f cicd/deploy/kubernetes/controller/optional/networkpolicy.yaml
```

> **Warning**: as written, this policy adds an Ingress `policyTypes` entry, which causes
> Kubernetes to **default-deny all other ingress** on the controller pod, including metrics
> scraping (port 12190) and kubelet liveness/readiness probes (port 12100). Before applying,
> edit the policy to add explicit ingress rules for those ports (or for the specific scraper
> and kubelet source CIDRs), or probe/metrics traffic will be blocked.

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
