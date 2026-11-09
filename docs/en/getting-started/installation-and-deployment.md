# Installation and Deployment

This page answers one question first: which Edgion deployment path should you start with?

## Choose a deployment mode

Gateway requires lock-free 128-bit atomic operations for retry-budget accounting.
On amd64, the CPU (including a VM's exposed CPU features) must support CMPXCHG16B.
Supported arm64 hosts provide the native atomic implementation. Gateway checks this
capability before starting services and exits with an error if it is unavailable;
it does not fall back to locks. This requirement applies even when no retry budget
is currently configured, because policies can be added through hot reload.
Controller and edgion-cli do not have this requirement.

Edgion currently has two primary paths:

1. **Kubernetes mode**
   Best when you already use Gateway API in a cluster and want CRD-driven configuration managed by the controller.
2. **Standalone mode**
   Best for local development, single-node debugging, bare-metal, or VM environments where you want to run Edgion as local processes with local configuration.

As a practical default:

- If you already have a Kubernetes cluster, start with Kubernetes mode.
- If you want to debug code or validate configuration locally first, start with standalone mode.

## Kubernetes quick entry

The repository ships a one-shot deployment script that installs the CRDs, controller, gateway, and base configuration:

```bash
cicd/deploy/kubernetes/scripts/deploy.sh -y
```

If you only need the CRDs first, run:

```bash
cicd/deploy/kubernetes/scripts/install_crds.sh
```

Before deploying, it is worth checking:

- Gateway API CRDs are available in the cluster
- you have `kubectl` access to the target namespace(s)
- image, RBAC, and environment assumptions are prepared according to the deployment docs

Recommended next reading:

- [Operations Guide / Gateway Resource Overview](../ops-guide/gateway/overview.md)
- [Operations Guide / GatewayClass Configuration](../ops-guide/gateway/gateway-class.md)
- [User Guide / HTTPRoute Overview](../user-guide/http-route/overview.md)

## Standalone quick entry

The repository ships a start script that launches the controller and gateway as local processes with the bundled file-system config:

```bash
cicd/deploy/standalone/start.sh
```

By default it uses `/usr/local/edgion` as the working directory; pass `--work-dir <path>` to change it, and `--foreground` to keep both processes attached to the terminal.

This mode is a good fit for:

- local development and debugging
- file-system configuration workflows
- single-node environments that do not depend on the Kubernetes API

If you need to understand process-level configuration and working directories, continue with:

- [Developer Guide / Work Directory Design](../dev-guide/work-directory.md)

When building any of the three binaries from source, select the allocator with
`allocator-jemalloc` (the default), `allocator-mimalloc`, or `allocator-system`.
The dependency crate names `tikv-jemallocator` and `mimalloc` are not Cargo
feature names for these binaries.

The Gateway also has optional subsystem features: `kafka`, `otlp`, `geoip`,
and `wasm`. The aggregate `full` feature selects all of them and is part
of the default set. When you build with `--no-default-features` and an explicit
feature list, add `full` (or the individual subsystems you need); otherwise they
are compiled out. A compiled-out subsystem never fails open: a primary Kafka/OTLP
log output, or an OTLP SSL-log output, stops Gateway startup (a fallback output is
skipped with a warning),
`LdapAuth` and `Wasm` plugin entries are rejected, and GeoIP lookups report the
database as unavailable so each `GeoIpLocation` plugin's `failOpen` setting
decides. `edgion-gateway --print-build-capabilities` lists which subsystems a
binary contains.

### Running the container image in a non-Kubernetes mode

The `start.sh` path above uses the release binaries with the CRD schemas already on disk under
`config/crd`, so nothing extra is required.

If you instead run the **controller container image** in a non-Kubernetes config-center mode
(`conf_center.type: file_system` or `etcd`), be aware that the runtime image does **not** bundle the
CRD schemas — they are only needed in non-Kubernetes mode, and in Kubernetes mode the schema comes
from the API server. Without them the controller exits at startup with `FATAL: No schemas loaded in
non-K8s mode`. Mount the CRD directory into `<work_dir>/config/crd`, for example:

```bash
docker run \
  -v "$PWD/config/crd:/usr/local/edgion/config/crd:ro" \
  -v "$PWD/edgion-controller.yaml:/usr/local/edgion/edgion-controller.yaml:ro" \
  docker.io/pandaala/edgion-controller:<tag> \
  --config-file /usr/local/edgion/edgion-controller.yaml
```

The gateway image never loads CRD schemas, so this applies to the controller only.

## Configuration and example entry points

The repository already contains a few useful starting points:

- `config/edgion-controller.yaml`
- `config/edgion-gateway.yaml`
- `edgion-tests/k8s/conf/`
- `edgion-tests/integration/conf/`

A practical reading pattern is:

- Kubernetes examples: `edgion-tests/k8s/conf/`
- local integration and file-system mode: `edgion-tests/integration/conf/`
- process-level configuration: `config/*.yaml`

## Admin API authentication

Authentication is **mandatory** for the Controller and Center Admin APIs — all `/api/v1/*` business routes require a valid credential.

### Controller

The Controller Admin API uses **token-only** (Istio-style) authentication. A pre-shared
admin token must be supplied explicitly — the Controller no longer auto-generates one. The
recommended Kubernetes approach is to run `deploy.sh`, which idempotently creates the
`edgion-admin-credentials` Secret (single `token` key) and wires it into the controller
Deployment as the `EDGION_ADMIN_TOKEN` env var.

Retrieve the token after running `deploy.sh`:

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

Then authorize the CLI with it:
```bash
edgion-cli login --token <TOKEN>
```

When no admin token (and no `cli_tokens`) is configured, the server starts but all Admin API
business routes return **503 fail-close** until a token is provided. See
[Controller Configuration](../ops-guide/configuration/controller-config.md#authentication)
for the full token bootstrap options.

### Center

Center has **no** auto-generation. The operator must provide either `local_auth` or `auth` in the Center configuration. If neither is configured:

- `/health`, `/ready`, `/metrics`, and `/api/v1/auth/status` remain accessible.
- All other business routes return **503 fail-close** until credentials are configured.

Setting `enabled: false` is **ignored** with a WARN log — authentication cannot be disabled.

For full configuration details see the [Controller Configuration reference](../ops-guide/configuration/controller-config.md#authentication).

## What to do next

If deployment is done and you want the smallest usable object set, continue with:

- [Your First Gateway](./first-gateway.md)

If you still need the mental model first, read:

- [Core Concepts](./core-concepts.md)
