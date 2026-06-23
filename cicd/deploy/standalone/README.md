# Standalone Deployment Guide

Deploy Edgion API Gateway on bare-metal servers or VMs without Kubernetes.

In standalone mode, the controller uses a **file-system config center** — it watches a local directory of Gateway API YAML files instead of the Kubernetes API. The same binary, same plugin system, same routing engine.

## Prerequisites

- Linux (x86_64 or aarch64) — tested on Ubuntu 22.04+, Debian 12+, CentOS 9+
- Edgion binaries: `edgion-controller`, `edgion-gateway`, `edgion-cli`

## Quick Start

```bash
# Start controller + gateway with default config
cicd/deploy/standalone/start.sh --work-dir /usr/local/edgion
```

## Installation

### Option 1: Download Pre-built Binaries

```bash
VERSION=0.1.5
ARCH=$(uname -m)  # x86_64 or aarch64

# Download and install
curl -L https://github.com/Pandaala/Edgion/releases/download/v${VERSION}/edgion-${VERSION}-${ARCH}.tar.gz \
  | tar xz -C /usr/local/bin/

# Verify
edgion-controller --version
edgion-gateway --version
```

### Option 2: Build from Source

```bash
# Default build (BoringSSL)
cargo build --release

# With OpenSSL
cargo build --release --no-default-features --features "allocator-jemalloc,openssl"

# With rustls
cargo build --release --no-default-features --features "allocator-jemalloc,rustls"

# Binaries output to target/release/
ls target/release/edgion-{controller,gateway,ctl}
```

## Directory Layout

```
/usr/local/edgion/              # work_dir
├── bin/                        # (optional) binaries
│   ├── edgion-controller
│   ├── edgion-gateway
│   └── edgion-cli
├── config/                     # YAML configuration
│   ├── edgion-controller.yaml
│   └── edgion-gateway.yaml
├── conf/                       # Gateway API YAML resources
│   └── gateway.yaml            # GatewayClass, Gateway, etc.
├── logs/                       # Log output
│   ├── edgion-controller.*.log
│   ├── edgion-gateway.*.log
│   ├── edgion_access.log
│   └── ssl.log
├── controller.pid
└── gateway.pid
```

## Admin Credentials

The controller no longer auto-generates admin credentials at startup. You must supply them
explicitly before using the Admin API. Two options:

**Option A — `[local_auth]` in the config file** (recommended for standalone):

```yaml
# edgion-controller.yaml
local_auth:
  enabled: true
  username: admin
  password: "<your-password>"
  jwt_secret: "<random string, at least 32 characters>"
  jwt_expiry_hours: 24
```

**Option B — environment variables** (12-factor style):

```bash
export EDGION_ADMIN_PASSWORD="$(openssl rand -base64 16)"
export EDGION_ADMIN_JWT_SECRET="$(openssl rand -base64 32)"
# EDGION_ADMIN_USERNAME defaults to "admin" if not set
./start.sh --work-dir /usr/local/edgion
```

If neither is provided, the server starts successfully but all Admin API business routes
return `503 Service Unavailable` (fail-close) until credentials are configured and the
controller is restarted. `/health`, `/ready`, and `/metrics` remain accessible.

> **Do not put the password in a ConfigMap.** Use the config file or env vars — both keep
> the secret out of version control and Kubernetes ConfigMap storage.

## Configuration

### Controller (`edgion-controller.yaml`)

```yaml
work_dir: "/usr/local/edgion"

server:
  grpc_listen: "0.0.0.0:12151"    # gRPC for gateway sync
  admin_listen: "0.0.0.0:12101"   # Admin API (config apply, /api/v1, /configclient)
  # probe_listen: "0.0.0.0:12100"   # Liveness/readiness (/health, /ready); default 0.0.0.0:12100
  # metrics_listen: "0.0.0.0:12190" # Prometheus metrics (/metrics); default 0.0.0.0:12190

logging:
  log_dir: logs
  log_prefix: edgion-controller
  log_level: info
  json_format: true

# FileSystem mode: watch local YAML directory
conf_center:
  type: file_system
  conf_dir: conf

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200
```

Key difference from Kubernetes mode: `conf_center.type: file_system` replaces `kubernetes`. The controller watches the `conf_dir` directory for YAML files and hot-reloads on changes.

### Gateway (`edgion-gateway.yaml`)

```yaml
work_dir: "/usr/local/edgion"

gateway:
  server_addr: "http://127.0.0.1:12151"   # Controller gRPC address
  admin_listen: "127.0.0.1:12001"         # Admin API (loopback-only; /configclient, /server-info)
  # probe_listen: "0.0.0.0:12000"         # Liveness/readiness (/health, /ready); default 0.0.0.0:12000

logging:
  log_dir: logs
  log_prefix: edgion-gateway
  log_level: "info,pingora_proxy=error,pingora_core=error"

server:
  # threads: 4                  # Default: CPU core count
  # work_stealing: true
  # grace_period_seconds: 30
  # upstream_keepalive_pool_size: 128

access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      rotation:
        strategy: daily
        max_files: 7
```

### Gateway API Config (`conf/`)

Place standard Gateway API YAML files in the `conf/` directory. The controller watches for file changes and hot-reloads automatically.

```yaml
# conf/gateway.yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: public-gateway
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: edgion-default-config
---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gateway-80
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http
      protocol: HTTP
      port: 80
      allowedRoutes:
        namespaces:
          from: All
```

Add HTTPRoute, GRPCRoute, TCPRoute, etc. as additional YAML files in the same directory.

## Manual Start/Stop

### Start

```bash
# Controller
edgion-controller \
  --config-file /usr/local/edgion/config/edgion-controller.yaml \
  --work-dir /usr/local/edgion \
  --conf-dir /usr/local/edgion/conf

# Gateway (after controller is healthy)
edgion-gateway \
  --config-file /usr/local/edgion/config/edgion-gateway.yaml \
  --work-dir /usr/local/edgion
```

### Stop

```bash
cicd/deploy/standalone/stop.sh --work-dir /usr/local/edgion
```

## Managing Routes

Use `edgion-cli` to apply configuration changes without restarting:

```bash
# Apply a new route
edgion-cli --server http://127.0.0.1:12101 apply -f my-route.yaml

# Or simply place YAML files in the conf/ directory — the controller watches for changes
cp my-route.yaml /usr/local/edgion/conf/
```

## Systemd Service (Production)

For production deployments, create systemd unit files:

```ini
# /etc/systemd/system/edgion-controller.service
[Unit]
Description=Edgion Controller
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/edgion-controller \
  --config-file /usr/local/edgion/config/edgion-controller.yaml \
  --work-dir /usr/local/edgion
Restart=always
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```ini
# /etc/systemd/system/edgion-gateway.service
[Unit]
Description=Edgion Gateway
After=edgion-controller.service
Requires=edgion-controller.service

[Service]
Type=simple
ExecStart=/usr/local/bin/edgion-gateway \
  --config-file /usr/local/edgion/config/edgion-gateway.yaml \
  --work-dir /usr/local/edgion
Restart=always
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now edgion-controller edgion-gateway
```

## Ports

| Service | Port | Bind | Description |
|---------|------|------|-------------|
| Controller gRPC | 12151 | 0.0.0.0 | Gateway config sync |
| Controller Admin | 12101 | 0.0.0.0 | Config apply, `/api/v1`, `/configclient` |
| Controller Probe | 12100 | 0.0.0.0 | Liveness/readiness (`/health`, `/ready`) |
| Controller Metrics | 12190 | 0.0.0.0 | Prometheus metrics (`/metrics`) |
| Gateway HTTP | 80 | 0.0.0.0 | Default HTTP listener |
| Gateway HTTPS | 443 | 0.0.0.0 | Default HTTPS listener |
| Gateway Admin | 12001 | 127.0.0.1 (loopback) | Internal admin API (`/configclient`, `/api/v1/server-info`) |
| Gateway Probe | 12000 | 0.0.0.0 | Liveness/readiness (`/health`, `/ready`) |
| Gateway Metrics | 12090 | 0.0.0.0 | Prometheus metrics |

## Troubleshooting

```bash
# Check controller health (probe listener)
curl http://127.0.0.1:12100/health

# Check gateway readiness
curl http://127.0.0.1:12000/ready

# View controller logs
tail -f /usr/local/edgion/logs/edgion-controller.*.log

# View gateway access logs
tail -f /usr/local/edgion/logs/edgion_access.log

# Inspect current config via edgion-cli
edgion-cli --server http://127.0.0.1:12101 get routes
```
