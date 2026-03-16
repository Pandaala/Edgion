# Standalone Deployment Guide

Deploy Edgion API Gateway on bare-metal servers or VMs without Kubernetes.

In standalone mode, the controller uses a **file-system config center** — it watches a local directory of Gateway API YAML files instead of the Kubernetes API. The same binary, same plugin system, same routing engine.

## Prerequisites

- Linux (x86_64 or aarch64) — tested on Ubuntu 22.04+, Debian 12+, CentOS 9+
- Edgion binaries: `edgion-controller`, `edgion-gateway`, `edgion-ctl`

## Quick Start

```bash
# Start controller + gateway with default config
deploy/standalone/start.sh --work-dir /usr/local/edgion
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
│   └── edgion-ctl
├── config/                     # TOML configuration
│   ├── edgion-controller.toml
│   └── edgion-gateway.toml
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

## Configuration

### Controller (`edgion-controller.toml`)

```toml
work_dir = "/usr/local/edgion"

[server]
grpc_listen = "0.0.0.0:50051"    # gRPC for gateway sync
admin_listen = "0.0.0.0:5800"    # Admin API (health, apply)

[logging]
log_dir = "logs"
log_prefix = "edgion-controller"
log_level = "info"
json_format = true

# FileSystem mode: watch local YAML directory
[conf_center]
type = "file_system"
conf_dir = "conf"

[conf_sync]
no_sync_kinds = ["ReferenceGrant", "Secret"]
default_capacity = 200
```

Key difference from Kubernetes mode: `conf_center.type = "file_system"` replaces `"kubernetes"`. The controller watches the `conf_dir` directory for YAML files and hot-reloads on changes.

### Gateway (`edgion-gateway.toml`)

```toml
work_dir = "/usr/local/edgion"

[gateway]
server_addr = "http://127.0.0.1:50051"   # Controller gRPC address
admin_listen = "0.0.0.0:5900"            # Gateway admin API

[logging]
log_dir = "logs"
log_prefix = "edgion-gateway"
log_level = "info,pingora_proxy=error,pingora_core=error"

[server]
# threads = 4                  # Default: CPU core count
# work_stealing = true
# grace_period_seconds = 30
# upstream_keepalive_pool_size = 128

[access_log]
enabled = true

[access_log.output.localFile]
path = "logs/edgion_access.log"

[access_log.output.localFile.rotation]
strategy = "daily"
max_files = 7
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
  --config-file /usr/local/edgion/config/edgion-controller.toml \
  --work-dir /usr/local/edgion \
  --conf-dir /usr/local/edgion/conf

# Gateway (after controller is healthy)
edgion-gateway \
  --config-file /usr/local/edgion/config/edgion-gateway.toml \
  --work-dir /usr/local/edgion
```

### Stop

```bash
deploy/standalone/stop.sh --work-dir /usr/local/edgion
```

## Managing Routes

Use `edgion-ctl` to apply configuration changes without restarting:

```bash
# Apply a new route
edgion-ctl --server http://127.0.0.1:5800 apply -f my-route.yaml

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
  --config-file /usr/local/edgion/config/edgion-controller.toml \
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
  --config-file /usr/local/edgion/config/edgion-gateway.toml \
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

| Service | Port | Description |
|---------|------|-------------|
| Controller gRPC | 50051 | Gateway config sync |
| Controller Admin | 5800 | Health check, config apply |
| Gateway HTTP | 80 | Default HTTP listener |
| Gateway HTTPS | 443 | Default HTTPS listener |
| Gateway Admin | 5900 | Readiness, runtime info |
| Gateway Metrics | 5901 | Prometheus metrics |

## Troubleshooting

```bash
# Check controller health
curl http://127.0.0.1:5800/health

# Check gateway readiness
curl http://127.0.0.1:5900/ready

# View controller logs
tail -f /usr/local/edgion/logs/edgion-controller.*.log

# View gateway access logs
tail -f /usr/local/edgion/logs/edgion_access.log

# Inspect current config via edgion-ctl
edgion-ctl --server http://127.0.0.1:5800 get routes
```
