# Installation and Deployment

This page answers one question first: which Edgion deployment path should you start with?

## Choose a deployment mode

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

## Configuration and example entry points

The repository already contains a few useful starting points:

- `config/edgion-controller.yaml`
- `config/edgion-gateway.yaml`
- `examples/k8stest/conf/`
- `edgion-tests/conf/`

A practical reading pattern is:

- Kubernetes examples: `examples/k8stest/conf/`
- local integration and file-system mode: `edgion-tests/conf/`
- process-level configuration: `config/*.yaml`

## Admin API authentication

Authentication is **mandatory** for the Controller and Center Admin APIs — all `/api/v1/*` business routes require a valid credential.

### Controller

Credentials must be supplied explicitly — the Controller no longer auto-generates them.
The recommended Kubernetes approach is to run `deploy.sh`, which idempotently creates the
`edgion-admin-credentials` Secret and wires it into the controller Deployment as env vars
(`EDGION_ADMIN_USERNAME`, `EDGION_ADMIN_PASSWORD`, `EDGION_ADMIN_JWT_SECRET`).

Retrieve the password after running `deploy.sh`:

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.password}' | base64 -d
```

Alternatively, authenticate interactively:
```bash
edgion-cli login
```

When no credentials are configured, the server starts but all Admin API business routes
return **503 fail-close** until credentials are provided. See
[Controller Configuration](../ops-guide/configuration/controller-config.md#authentication)
for the full credential bootstrap options.

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
