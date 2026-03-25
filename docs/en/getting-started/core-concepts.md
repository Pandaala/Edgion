# Core Concepts

This page is meant to establish the smallest useful Edgion mental model without diving into implementation details.

## 1. Controller and Gateway are separate

Edgion does not put everything into one process.

- **Controller**
  Reads configuration, validates resources, updates status, and syncs usable configuration to gateways.
- **Gateway**
  Receives traffic, performs route matching, runs plugins, handles TLS, and forwards requests.

If you only care about deployment and usage, this is enough for now. If you want implementation details, read:

- [Developer Guide / Architecture Overview](../dev-guide/architecture-overview.md)

## 2. GatewayClass decides who manages the gateway

`GatewayClass` is the control-plane entry point.

The most important field is:

```yaml
controllerName: edgion.io/gateway-controller
```

That tells Gateway API that Edgion is the controller responsible for this class.

## 3. Gateway defines traffic entry points

`Gateway` defines:

- which listeners exist
- which protocol each listener uses
- which port it listens on
- whether TLS is required
- which routes are allowed to bind

You can think of it as the traffic entry declaration.

## 4. Route resources define matching and forwarding

Different protocols use different route resources:

- `HTTPRoute`
- `GRPCRoute`
- `TCPRoute`
- `UDPRoute`
- `TLSRoute`

Together, these resources answer:

- which requests should match
- what processing should happen after a match
- where traffic should finally be forwarded

## 5. Edgion adds extensions on top of Gateway API

Besides standard Gateway API resources, Edgion provides extensions such as:

- `EdgionPlugins`
- `EdgionStreamPlugins`
- `EdgionTls`
- `EdgionGatewayConfig`

These are typically used for:

- HTTP, TCP, and TLS plugins
- finer-grained TLS behavior
- gateway-level advanced configuration

When you see the `🔌 Edgion Extension` marker in the docs, that feature is not part of standard Gateway API.

## 6. Recommended learning order

If this is your first time with Edgion, a practical order is:

1. [Your First Gateway](./first-gateway.md)
2. [Operations Guide / Gateway Resource Overview](../ops-guide/gateway/overview.md)
3. [User Guide / HTTPRoute Overview](../user-guide/http-route/overview.md)
4. Then continue into TLS, filters, plugins, or backend configuration depending on your use case

If you are already debugging implementation details or planning to extend the system, switch to:

- [Developer Guide](../dev-guide/README.md)
