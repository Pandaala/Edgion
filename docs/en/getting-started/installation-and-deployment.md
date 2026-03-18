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

The repository root README currently points to this shortest path:

```bash
deploy/kubernetes/scripts/deploy.sh -y
```

This path installs the CRDs, controller, gateway, and base configuration.

Before doing that, it is worth checking:

- Gateway API CRDs are available in the cluster
- you have `kubectl` access to the target namespace(s)
- image, RBAC, and environment assumptions are prepared according to the deployment docs

Recommended next reading:

- [Operations Guide / Gateway Resource Overview](../ops-guide/gateway/overview.md)
- [Operations Guide / GatewayClass Configuration](../ops-guide/gateway/gateway-class.md)
- [User Guide / HTTPRoute Overview](../user-guide/http-route/overview.md)

## Standalone quick entry

The repository root README currently points to this shortest path:

```bash
deploy/standalone/start.sh
```

This mode is a good fit for:

- local development and debugging
- file-system configuration workflows
- single-node environments that do not depend on the Kubernetes API

If you need to understand process-level configuration and working directories, continue with:

- [Developer Guide / Work Directory Design](../dev-guide/work-directory.md)

## Configuration and example entry points

The repository already contains a few useful starting points:

- `config/edgion-controller.toml`
- `config/edgion-gateway.toml`
- `examples/k8stest/conf/`
- `examples/test/conf/`

A practical reading pattern is:

- Kubernetes examples: `examples/k8stest/conf/`
- local integration and file-system mode: `examples/test/conf/`
- process-level configuration: `config/*.toml`

## What to do next

If deployment is done and you want the smallest usable object set, continue with:

- [Your First Gateway](./first-gateway.md)

If you still need the mental model first, read:

- [Core Concepts](./core-concepts.md)
