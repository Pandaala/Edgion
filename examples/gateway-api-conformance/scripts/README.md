# Conformance Scripts

This directory contains the full set of operation scripts for Gateway API conformance testing.

## Script overview

| Script | Purpose | Typical scenario |
|--------|---------|------------------|
| `setup-orbstack.sh` | OrbStack-only: local image build + full deployment | **Preferred for daily development** |
| `orbstack-dev.sh` | OrbStack dev toolbox: build/deploy/hot reload/logs/status | Fast iteration after code changes |
| `run.sh` | Run conformance tests | Each verification run |
| `setup.sh` | Generic deployment: no image build, suitable for CI or remote clusters | CI / production clusters |
| `cleanup.sh` | Clean up the test environment (does not remove CRDs) | Environment reset |

---

## Common workflows

### First-time environment setup (OrbStack)

```bash
# Build images + deploy all components + wait for readiness
./scripts/setup-orbstack.sh

# Run a smoke test to confirm the environment is healthy
./scripts/orbstack-dev.sh test --run-test HTTPRouteSimpleSameNamespace
```

### Updating after code changes

```bash
# Approach 1: standard (Docker build + rollout)
./scripts/orbstack-dev.sh rebuild        # controller + gateway
./scripts/orbstack-dev.sh rebuild g      # update gateway only
./scripts/orbstack-dev.sh rebuild c      # update controller only

# Approach 2: fast hot reload (cargo zigbuild local compile -> kubectl cp -> pkill, takes seconds)
# Prerequisites: brew install zig && cargo install cargo-zigbuild && rustup target add aarch64-unknown-linux-gnu
./scripts/orbstack-dev.sh localpatch g   # hot reload gateway only
./scripts/orbstack-dev.sh localpatch c   # hot reload controller only
./scripts/orbstack-dev.sh localpatch     # hot reload both
```

### Running conformance tests

```bash
# OrbStack full run — use run.sh directly, no port-forward needed
# OrbStack natively allows macOS to access K8s ClusterIPs; direct connection is the most stable.
# If routes are lost after an OrbStack restart, run.sh detects this automatically and recovers
# via the node IP — no manual action needed.
bash scripts/run.sh

# Single test
bash scripts/run.sh --run-test HTTPRouteSimpleSameNamespace

# Use GO_RUN_FILTER for more flexible regex matching
GO_RUN_FILTER="TestConformance/HTTPRoute.*Redirect" bash scripts/run.sh

# Non-OrbStack clusters (port-forward is only needed when Pod IPs are unreachable)
bash scripts/run.sh --port-forward

# TLSRoute-only (initialize the environment with --tls-only first; see below)
GO_RUN_FILTER="TestConformance/TLSRouteSimpleSameNamespace$" \
  bash scripts/run.sh --conformance-profiles GATEWAY-TLS
```

> **Warning**: `orbstack-dev.sh test` always adds `--port-forward` internally.
> For full runs, use `bash scripts/run.sh` directly to avoid the port-forward dropping during
> long runs (~12 minutes), which would cause many `connection refused` failures.

### TLSRoute test environment

The TLSRoute 443 passthrough listener conflicts with the default HTTPS listener and requires a separate environment:

```bash
# First, re-initialize into TLS-only mode
./scripts/setup-orbstack.sh --tls-only --skip-build

# Then run the TLSRoute tests
./scripts/run.sh --conformance-profiles GATEWAY-TLS
```

### Checking status and logs

```bash
./scripts/orbstack-dev.sh status          # Pod / GatewayClass / Gateway status
./scripts/orbstack-dev.sh logs g          # follow gateway logs
./scripts/orbstack-dev.sh logs c          # follow controller logs
./scripts/orbstack-dev.sh shell g         # enter the gateway pod shell
./scripts/orbstack-dev.sh shell c         # enter the controller pod shell
```

### Cleaning up the environment

```bash
./scripts/cleanup.sh   # Remove the conformance namespace + Edgion components, keep CRDs
```

---

## Per-script details

### `setup-orbstack.sh` — full OrbStack initialization

**What it does**: an 8-step end-to-end deployment flow tailored for OrbStack local development:

1. Verify the kubectl context is `orbstack`.
2. Build local images with `docker build` (tag: `pandaala/edgion-*:conformance-dev`, `imagePullPolicy: Never`).
3. Install namespaces + CRDs (Gateway API experimental v1.5.0 + Edgion CRDs).
4. Deploy the controller (with `RUST_BACKTRACE` injected).
5. Apply GatewayClass + EdgionGatewayConfig.
6. Pre-create the Gateway CRs needed by conformance (avoiding races when the test framework creates them).
7. Deploy the gateway data plane (restart so it loads the pre-created listeners).
8. Auto-correct `gateway_address` (read the edgion-gateway Service ClusterIP, write it into the controller ConfigMap, restart the controller, wait for `Gateway status.addresses` to update).

**Key flags**:

```bash
./scripts/setup-orbstack.sh                # Full run (debug build)
./scripts/setup-orbstack.sh --skip-build   # Skip build, deploy directly (when the image already exists)
./scripts/setup-orbstack.sh --rebuild      # Force a no-cache image rebuild
./scripts/setup-orbstack.sh --release      # Release build (faster, no backtrace)
./scripts/setup-orbstack.sh --tls-only     # TLSRoute-only environment (no HTTPS listener)
```

**Why the gateway_address fix-up step exists**: the conformance framework reads the target IP from `Gateway.status.addresses` to send requests. The Edgion controller needs to know the gateway Service ClusterIP before it can write status, but the ClusterIP is only assigned after the Service is created. The setup script therefore writes the ConfigMap and restarts the controller once the Service is ready.

---

### `orbstack-dev.sh` — development iteration toolbox

**What it does**: the most-used tool during daily development, wrapping every subcommand from build to debugging.

**Command quick reference**:

| Command | Description |
|---------|-------------|
| `build [c\|g\|all]` | `docker build` only, no rollout |
| `deploy [c\|g\|all]` | Rollout only (update image tag + restart), no build |
| `rebuild [c\|g\|all]` | `build` + `deploy` in one step |
| `localpatch [c\|g\|all]` | **Fastest**: local zigbuild compile -> `kubectl cp` -> `pkill`, no Docker |
| `hotpatch [c\|g]` | Extract binary from Docker image -> `kubectl cp` -> `pkill` |
| `test [run.sh args]` | Pass-through to `run.sh --port-forward` (auto port-forward mode) |
| `status` | Print a summary of Pod / GatewayClass / Gateway status |
| `logs [c\|g]` | `kubectl logs -f` |
| `restart [c\|g\|all]` | `kubectl rollout restart` |
| `shell [c\|g]` | `kubectl exec -it ... /bin/sh` |

**localpatch prerequisites** (one-time setup):

```bash
brew install zig
cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu
```

**PROFILE environment variable**:

```bash
PROFILE=release ./scripts/orbstack-dev.sh rebuild   # release build
# Default is debug (RUST_BACKTRACE=full)
```

---

### `run.sh` — run the conformance tests

**What it does**: invokes `go test` to run the Gateway API conformance suite, with profile filtering, single-test runs, and auto port-forward.

Each invocation runs the following **prechecks**; any failure exits early with a clear message instead of letting the tests run before failing:

| Check | Description |
|-------|-------------|
| Leftover processes | Detects `gateway-api-conformance.test` and `kubectl port-forward` processes left over from previous runs, and suggests pkill commands |
| Go toolchain | Confirms that `go` is in PATH |
| Cluster connectivity | `kubectl cluster-info` quickly validates that kubeconfig works |
| OrbStack routing | Probes ClusterIP reachability; if routing has failed, automatically locates the OrbStack node IP and enables the TAL proxy (see below) |
| Gateway pod Ready | At least one `edgion-gateway` pod is in Ready state |
| GatewayClass Accepted | The `edgion` GatewayClass has `Accepted` condition True |

**Key flags**:

```bash
./scripts/run.sh                                         # Default: GATEWAY-HTTP + GATEWAY-GRPC
./scripts/run.sh --conformance-profiles "GATEWAY-HTTP"   # HTTP profile only
./scripts/run.sh --run-test HTTPRouteHeaderMatching       # Single test
./scripts/run.sh --core                                   # Core features only
./scripts/run.sh --extended                               # Core + Extended features
./scripts/run.sh --port-forward                           # Auto port-forward (non-direct clusters)
./scripts/run.sh --no-debug                               # Disable debug output (less log noise)
./scripts/run.sh --timeout 10m                            # Custom timeout
```

**Transport Access Layer (TAL) environment variables**:

```bash
# Manual control of port-forward mode
export GATEWAY_PROXY_HOST=127.0.0.1
export GATEWAY_PROXY_PORT_MAP="80:18080,443:18443,8443:28443"   # recommended
# Or set them individually
export GATEWAY_PROXY_HTTP_PORT=18080
export GATEWAY_PROXY_HTTPS_PORT=18443
export GATEWAY_PROXY_HTTPS_8443_PORT=28443

export GATEWAY_PROXY_ENABLE_GRPC=true   # Also enable the gRPC proxy client
export TAL_DEBUG=true                    # Log every address rewrite
```

**Output**: after the run, a YAML conformance report is generated at `conformance-report.yaml` (project root).

---

### `setup.sh` — generic deployment (CI / remote clusters)

**What it does**: the same 8-step deployment as `setup-orbstack.sh`, but **without building images**. Suitable for:

- CI pipelines (images pushed to a registry by CD).
- Remote clusters (images pulled from a registry).

**Main differences from `setup-orbstack.sh`**:

| | `setup-orbstack.sh` | `setup.sh` |
|---|---|---|
| Builds images | Yes (docker build) | No |
| imagePullPolicy | `Never` | `Always` (default) |
| Target environment | OrbStack local | CI / remote clusters |
| context check | Yes (checks orbstack) | No |

```bash
./scripts/setup.sh
./scripts/setup.sh --controller-image registry.example.com/edgion-controller:v1.0.0
./scripts/setup.sh --gateway-image registry.example.com/edgion-gateway:v1.0.0
./scripts/setup.sh --pull-policy IfNotPresent
./scripts/setup.sh --tls-only    # TLSRoute-only mode
./scripts/setup.sh --skip-crd    # Skip CRD installation (when already installed)
```

---

### `cleanup.sh` — clean up the environment

**What it does**: removes everything created by the conformance tests, while keeping CRDs and the `edgion-system` namespace.

Concrete deletion scope:
- `gateway-conformance-infra` / `gateway-conformance-app-backend` / `gateway-conformance-web-backend` namespaces
- `GatewayClass edgion`
- `EdgionGatewayConfiguration conformance-gateway`
- The controller / gateway Deployments, Services, and ConfigMaps under `edgion-system`

**Not removed** (manual cleanup required):
- The `edgion-system` namespace itself
- Gateway API CRDs
- Edgion CRDs

```bash
./scripts/cleanup.sh

# For full removal (including CRDs):
kubectl delete namespace edgion-system
kubectl delete -f config/crd/gateway-api/
kubectl delete -f config/crd/edgion-crd/
```

---

## Notes

### Pre-created Gateways may be lost

`setup-orbstack.sh` pre-creates 6 Gateway CRs (`same-namespace`, `same-namespace-with-https-listener`, `all-namespaces`, `backend-namespaces`, `client-validation-default`, `client-validation-insecure-fallback`).

If you have run `cleanup.sh` or deleted them manually, some Gateways may be missing. To restore:

```bash
kubectl apply --server-side --force-conflicts -f manifests/conformance-gateway.yaml
# Restart the gateway to bind the new ports (e.g. 8443)
./scripts/orbstack-dev.sh restart g
```

### go test output is fully buffered

A full run takes about 12 minutes, during which the terminal **emits no output** (go test buffers all sub-package `-v` output and only prints it after the tests finish). This is normal — do not assume the run has hung.

### Conformance report pitfall in single-test mode

When running a single test with `--run-test` or `GO_RUN_FILTER`, the generated `conformance-report.yaml` marks every test that **was not run** as Passed. This does not represent a full pass; it is just how the conformance framework behaves. Only a report from a full run reflects reality.

---

## FAQ

**Q: Why does `setup-orbstack.sh` correct gateway_address manually?**

The Edgion controller writes `gateway_address` (the edgion-gateway Service ClusterIP) into `Gateway.status.addresses`, which the conformance framework uses to send test requests. The ClusterIP does not exist before the Service is created, so the setup script must read and inject it once the Service is created.

**Q: Why pre-create the conformance Gateway CRs?**

The Edgion gateway process binds listener ports at startup and does not support adding ports dynamically afterwards. The conformance framework creates Gateway CRs during the tests; if the gateway process is already running, new listeners cannot bind. The setup script therefore pre-creates the Gateway CRs so the gateway loads the full listener configuration on startup.

**Q: Why does TLSRoute need its own environment (`--tls-only`)?**

The default environment deploys an HTTPS listener on port 443 (for the `HTTPRouteHTTPS*` tests). TLSRoute needs TLS passthrough on the same port 443; the two conflict. `--tls-only` skips the HTTPS listener and only creates a TLS passthrough Gateway.

**Q: Under OrbStack, should I use `orbstack-dev.sh test` or `run.sh`?**

- **Single test**: `orbstack-dev.sh test --run-test <Name>` is convenient; the port-forward duration is short and won't drop.
- **Full run**: use `bash scripts/run.sh` directly. `orbstack-dev.sh test` always adds `--port-forward`, which tends to drop during long runs.

**Q: After an OrbStack restart, all tests time out and report unreachable ClusterIPs. What now?**

After an OrbStack restart, route entries for the K8s pod/service CIDR (`192.168.194.x`) may be missing from the Mac routing table, so ClusterIP traffic leaks to the physical gateway.

`run.sh` detects this automatically: after a failed ClusterIP probe, it queries the OrbStack built-in DNS (`198.18.0.2`) for `orbstack-k8s.internal` to obtain the node IP, confirms the node is reachable, and enables the TAL proxy to bypass the missing routes so the tests run normally.

If the OrbStack node itself is unreachable, the script prompts you to restart OrbStack.app. Permanent route recovery happens automatically after OrbStack.app restarts, or run manually:

```bash
# <orb_node_ip> is typically 198.18.0.37; confirm via script output
sudo route -n add -net 192.168.194.0/25 <orb_node_ip>
sudo route -n add -net 192.168.194.128/25 <orb_node_ip>
```
