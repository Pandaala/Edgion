# Standalone Deployment Guide

Deploy Edgion API Gateway on bare-metal servers or VMs without Kubernetes.

In standalone mode, the controller uses a **file-system config center** — it watches a local directory of Gateway API YAML files instead of the Kubernetes API. The same binary, same plugin system, same routing engine.

## Prerequisites

- Linux (x86_64 or aarch64) — tested on Ubuntu 22.04+, Debian 12+, CentOS 9+
- Edgion binaries: `edgion-controller`, `edgion-gateway`, `edgion-cli`

## Quick Start

```bash
# 1. Generate the conf_sync mTLS cert set (once). The shipped config defaults to file-mode
#    mTLS on the controller<->gateway channel, so this must exist before first start.
edgion-cli gen-certs --output file --output-dir /usr/local/edgion/certs/grpc \
    --server-san 127.0.0.1 --server-san localhost

# 2. Start controller + gateway with default config
cicd/deploy/standalone/start.sh --work-dir /usr/local/edgion
```

> The conf_sync channel carries resolved Secrets (TLS private keys, backend credentials)
> and is **mTLS by default**. Omitting `conf_sync_security` makes both binaries refuse to
> start. See [conf_sync channel security](#conf_sync-channel-security) for split-host setups.

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

# Binaries output to target/release/
ls target/release/edgion-{controller,gateway,ctl}
```

The Gateway rustls backend feature is not a supported source-build target.

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
  grpc_listen: "0.0.0.0:12151"    # gRPC for gateway sync — mTLS by default, safe on all interfaces
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

# REQUIRED — omitting this section makes the controller refuse to start.
# File-mode mTLS (default). Generate the set with `edgion-cli gen-certs` (see Quick Start).
conf_sync_security:
  tls:
    cert: certs/grpc/primary/server.crt
    key: certs/grpc/primary/server.key
    ca: certs/grpc/primary/ca.crt
```

Key difference from Kubernetes mode: `conf_center.type: file_system` replaces `kubernetes`. The controller watches the `conf_dir` directory for YAML files and hot-reloads on changes.

#### Etcd HA (multi-controller)

For a highly-available control plane without Kubernetes, switch `conf_center.type` to `etcd` and point
2+ controllers at a shared etcd cluster. Leader election (etcd `v3election` + a self-managed lease
keepalive) is **always on**: exactly one controller becomes leader (runs the controllers and serves
conf_sync); standbys reject traffic and return `409` naming the leader on the Admin API. Gateways list
every controller's address in `gateway.server_addrs` and rotate to the current leader. A commented-out
example lives in `edgion-controller.yaml`. Two caveats: etcd must expose the `v3election` gRPC service
(probed at startup), and conf_sync mTLS requires **manually-issued** certs whose server-cert SAN covers
**every** controller's `grpc_addr` (`auto_ca` is Kubernetes-only). See
[Controller Configuration → Etcd Mode](../../../docs/en/ops-guide/configuration/controller-config.md#conf_center--etcd-mode)
for the full field reference and failover behavior.

### Gateway (`edgion-gateway.yaml`)

```yaml
work_dir: "/usr/local/edgion"

gateway:
  server_addrs: ["https://127.0.0.1:12151"]  # Controller gRPC address (https under mTLS)
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

# REQUIRED — omitting this section makes the gateway refuse to start.
# File-mode mTLS (default), client side of the same set the controller uses.
conf_sync_security:
  tls:
    cert: certs/grpc/primary/client.crt
    key: certs/grpc/primary/client.key
    ca: certs/grpc/primary/ca.crt
```

### conf_sync channel security

The conf_sync gRPC channel (controller `12151` → gateway) carries **resolved Secrets** — TLS
private keys and backend credentials — from the controller to the gateway. Its transport must
be an explicit choice; omitting `conf_sync_security` makes **both binaries refuse to start**.

The shipped standalone configs default to **file-mode mTLS**, so the channel is protected whether
you run both processes on one host or split them across two. Generate the cert set once, before
first start:

```bash
edgion-cli gen-certs --output file --output-dir /usr/local/edgion/certs/grpc \
    --server-san 127.0.0.1 --server-san localhost
```

This writes `certs/grpc/primary/{ca,server,client}.{crt,key}` (paths are relative to `work_dir`).
The controller uses the `server.*` half, the gateway uses the `client.*` half, and both trust the
same `ca.crt`. The two sides must agree — a mismatch is a handshake failure.

**Split-host deployment.** The `--server-san` values must include whatever host/IP the gateway's
`server_addrs` dials (a cert with only `127.0.0.1`/`localhost` will fail SAN verification from a
remote gateway). Re-generate with the controller's reachable address, then distribute the client
half to the gateway host:

```bash
# On the controller host — bake the reachable address into the server cert:
edgion-cli gen-certs --output file --output-dir /usr/local/edgion/certs/grpc \
    --server-san controller.example.internal

# Copy client.crt / client.key / ca.crt to the gateway host, then on the gateway:
```

```yaml
# edgion-gateway.yaml
gateway:
  server_addrs: ["https://controller.example.internal:12151"]  # must match a --server-san above
conf_sync_security:
  tls:
    cert: certs/grpc/primary/client.crt
    key: certs/grpc/primary/client.key
    ca: certs/grpc/primary/ca.crt
```

`grpc_listen` is already `0.0.0.0` in the shipped controller config — no change is needed to move
the gateway to another host, because the mTLS handshake (not a loopback bind) is what keeps the
Secrets off the wire.

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
| Controller gRPC | 12151 | 127.0.0.1 (loopback) | Gateway config sync — plaintext by default, so it must not leave the host (see "conf_sync channel security") |
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
