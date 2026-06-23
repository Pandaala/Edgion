# Configuration Reference

Edgion has three distinct configuration surfaces. Decide which one you need to change before looking up its schema — they have different carriers, scopes, and reload semantics.

## Configuration Surfaces

| Surface | Carrier | Scope | How to Apply |
|---------|---------|-------|--------------|
| Controller process config | YAML file (`config/edgion-controller.yaml`) | Single Controller instance | Restart process (some sections hot-reload via `POST /api/v1/reload`) |
| Gateway process config | YAML file (`config/edgion-gateway.yaml`) | Single Gateway instance | Restart process |
| Runtime config | `EdgionGatewayConfig` CRD (`edgion.io/v1alpha1`) | Entire GatewayClass | Takes effect dynamically (no restart) |

Both process configs are loaded from a YAML file at startup, selectable with `--config-file`. The CRD is cluster-scoped and referenced from `GatewayClass.spec.parametersRef`.

> **Federation Center**: A fourth binary, Center, has its own YAML process config (`config/edgion-center.yaml`, also `--config-file`). It is only used in multi-cluster federation deployments. Unlike the Controller, Center never auto-generates admin credentials — you must configure `local_auth` or `auth`, or business routes fail closed with 503. To join a Controller to a Center, add the `center` section to the Controller config (see [controller-config.md](controller-config.md#federation-client-center)).

> All four surfaces are **YAML**. There is no TOML configuration anywhere in Edgion.

## Which Surface Do I Change?

| I want to change… | Surface |
|-------------------|---------|
| Controller gRPC / Admin listen address, `conf_center` mode, admin auth | Controller config |
| Which Controller a Gateway connects to, log paths, Pingora worker tuning, rate-limit precision | Gateway config |
| HTTP timeouts, retries, real-IP extraction, security policy, global plugins | `EdgionGatewayConfig` CRD |

## Contents

| Document | Description |
|----------|-------------|
| [controller-config.md](controller-config.md) | Controller YAML — full parameter reference (server, conf_center, auth, federation, CLI tokens) |
| [gateway-config.md](gateway-config.md) | Gateway YAML — full parameter reference (server, logging, business logs, rate limiting) |
| [edgion-gateway-config.md](edgion-gateway-config.md) | EdgionGatewayConfig CRD — GatewayClass-level runtime configuration |
