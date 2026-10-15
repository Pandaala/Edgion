# Standalone Deployment Guide

Deploy Edgion API Gateway on bare-metal servers or VMs without Kubernetes.

In standalone mode, the controller uses a **file-system config center** — it watches a local directory of Gateway API YAML files instead of the Kubernetes API. The same binary, same plugin system, same routing engine.

## Packages and Services

Standalone deployment assets are organized here:

```text
cicd/deploy/standalone/
  README.md           Deployment entry point
  packaging/          nFPM manifests, DEB/RPM build scripts, lifecycle hooks
  systemd/            Authoritative service units and optional drop-ins
  tests/              Disposable ARM64 package/service lifecycle tests
  edgion-*.yaml       Manual-deployment process configuration
  conf/               Manual-deployment resource examples
  start.sh, stop.sh   Manual-deployment process helpers
```

| Target | Package format | Service manager | Runtime validation |
|--------|----------------|-----------------|--------------------|
| Debian 12 | DEB | systemd | ARM64 Docker |
| Ubuntu 24.04 | DEB | systemd | ARM64 Docker |
| Rocky Linux 9 | RPM | systemd | ARM64 Docker |

Controller, Gateway, and CLI are independent packages. The complete package
lifecycle passed on all three ARM64 targets. An AMD64 build/packaging entry is
available without runtime validation; see [test scope and limits](tests/README.md).

- [Packaging](packaging/README.md): build, installation, initialization, upgrades,
  recovery, and removal.
- [Systemd](systemd/README.md): service identities, limits, shutdown budgets, and
  operator overrides.
- [Tests](tests/README.md): repeatable package/service lifecycle verification.
- [Linux tuning](../linux-tuning/README.md): optional inert tuning assets.

The [manual ARM64 workflow](../../../.github/workflows/standalone-packages.yml)
builds both formats, runs the matrix, and retains artifacts. Package publication
and repositories require separate release integration. Build outputs and test
reports belong in ignored work directories.

First installation leaves services stopped and disabled. Supply credentials,
certificates, and resource configuration before explicitly starting them. Package
hooks never tune the host or generate Admin Tokens. The examples below describe
manual deployment, whose paths differ from packaged installations.

## Prerequisites

- Manual deployment: a compatible Linux runtime and supplied product binaries.
- Package targets: Ubuntu 24.04, Debian 12 (DEB), and Rocky Linux 9 (RPM).
  See [test scope and limits](tests/README.md) for ARM64 validation coverage.
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

### Build and Install Packages

See [packaging commands and initialization](packaging/README.md). Build current
source binaries on the Rocky 9 baseline, then generate three independent DEB
and RPM packages. First installation leaves Controller and Gateway stopped and
disabled. Supply tokens and mTLS certificates before explicitly enabling them.
The CLI package has no daemon service.

Package configuration lives under `/etc/edgion/<role>/`, mutable work data under
`/var/lib/edgion/<role>/`, and logs under `/var/log/edgion/<role>/`. The directory
layout and YAML examples below describe manual deployment.

### Build from Source for Manual Deployment

```bash
cargo build --locked --profile production \
  -p edgion-gateway -p edgion-controller -p edgion-cli \
  --bin edgion-gateway --bin edgion-controller --bin edgion-cli
ls target/production/edgion-{controller,gateway,cli}
```

The Gateway uses the supported BoringSSL backend. A binary built on a newer
Linux distribution may require a newer glibc than another target provides.

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

Each daemon uses an operator-supplied pre-shared bearer token. Supply Controller
`EDGION_ADMIN_TOKEN` and Gateway `EDGION_GATEWAY_ADMIN_TOKEN` through protected
environment files for packaged services; see [initialization](packaging/README.md).
For manual processes, inject these variables from your secret-management system
or set the documented token fields in protected process configuration files.

CLI reads `EDGION_TOKEN` or its protected cached token. Keep tokens out of command
arguments, source control, logs, and shared files. There is no username/password
or admin JWT bootstrap. Packages never generate Admin Tokens.

When tokens are absent, business Admin API routes fail closed with HTTP 503;
health/readiness probes remain available. See the
[authentication contract](../../../skills/02-features/02-config/03-auth-bootstrap.md).

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
  controller_name: edgion.io/gateway-controller
  conf_dir: conf

conf_sync:
  no_sync_kinds: [ReferenceGrant, Secret]
  default_capacity: 200

# REQUIRED — omitting this section makes the controller refuse to start.
# File-mode mTLS (default). Generate the set with `edgion-cli gen-certs` (see Quick Start).
conf_sync_security:
  active: primary
  certs:
    - name: primary
      cert: certs/grpc/primary/server.crt
      key: certs/grpc/primary/server.key
      ca: certs/grpc/primary/ca.crt
```

Key difference from Kubernetes mode: `conf_center.type: file_system` replaces `kubernetes`. The controller watches the `conf_dir` directory for YAML files and hot-reloads on changes.

FileSystem and etcd require an explicit `conf_center.controller_name`; omission,
blank values or invalid Gateway API controller names fail startup. This example's
identity matches the supplied GatewayClass. Use distinct identities for independent
deployments and update their GatewayClasses to match.

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
  admin_listen: "127.0.0.1:12001"         # Admin API (loopback-only; /configclient)
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
  active: primary
  certs:
    - name: primary
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
  active: primary
  certs:
    - name: primary
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

Use `edgion-cli` to update routes on existing Gateway listeners without restarting:

```bash
# Apply a new route
edgion-cli --server http://127.0.0.1:12101 apply -f my-route.yaml

# Or simply place YAML files in the conf/ directory — the controller watches for changes
cp my-route.yaml /usr/local/edgion/conf/
```

Listener sockets are created at Gateway startup. Adding or removing listener
ports, or deleting and recreating a Gateway, requires a Gateway process restart.
Install the desired GatewayClass, Gateway and routes before starting Gateway;
when adding a port to a running deployment, wait for configuration sync and then
restart the Gateway. Route and certificate updates on existing listeners still
hot-reload. See the [startup contract](../../../skills/01-architecture/02-gateway/01-startup-shutdown.md).

## Systemd Services

Use the authoritative [Controller unit](systemd/edgion-controller.service) and
[Gateway unit](systemd/edgion-gateway.service) installed by the packages. They use
separate non-login identities, absolute package paths, graceful SIGTERM, and
bounded restart behavior. CLI has no service.

The base Gateway unit operates independently of a local Controller. The optional
[same-host drop-in](systemd/same-host.conf) adds startup ordering without
propagating Controller stops to Gateway. Install it only for a same-host topology.
See [service configuration](systemd/README.md) for operator overrides and stop
budgets. Optional host tuning remains a separate explicit action.

```bash
# After completing package initialization:
sudo systemctl enable --now edgion-controller
curl --fail http://127.0.0.1:12100/ready
sudo systemctl enable --now edgion-gateway
curl --fail http://127.0.0.1:12000/ready
```

Install the GatewayClass, Gateway and matching routing resources before starting
Gateway. Service activation and probe readiness do not prove that a newly added
port is bound; port-set changes require a Gateway restart. Verify a real proxy
request.

## Ports

| Service | Port | Bind | Description |
|---------|------|------|-------------|
| Controller gRPC | 12151 | 127.0.0.1 packaged; 0.0.0.0 manual example | Gateway config sync with mTLS by default |
| Controller Admin | 12101 | 127.0.0.1 packaged; 0.0.0.0 manual example | Config apply, `/api/v1`, `/configclient` |
| Controller Probe | 12100 | 127.0.0.1 packaged; 0.0.0.0 manual default | Liveness/readiness (`/health`, `/ready`) |
| Controller Metrics | 12190 | 127.0.0.1 packaged; 0.0.0.0 manual default | Prometheus metrics (`/metrics`) |
| Gateway HTTP | 80 | 0.0.0.0 | Resource-defined example listener |
| Gateway HTTPS | 443 | 0.0.0.0 | Resource-defined when an HTTPS listener is configured |
| Gateway Admin | 12001 | 127.0.0.1 (loopback) | Authenticated internal admin API (`/configclient`) |
| Gateway Probe | 12000 | 127.0.0.1 packaged; 0.0.0.0 manual default | Liveness/readiness (`/health`, `/ready`) |
| Gateway Metrics | 12090 | 127.0.0.1 packaged; 0.0.0.0 manual default | Prometheus metrics |

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
