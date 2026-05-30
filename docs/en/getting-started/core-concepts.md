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

## 3. Gateway and Listeners define traffic entry points

A `Gateway` is the traffic entry declaration. It is made up of one or more **listeners**, where each listener defines:

- which protocol it uses (HTTP, HTTPS, TLS, TCP, UDP)
- which port it listens on
- whether TLS is required
- which routes are allowed to bind

Routes attach to a specific listener by name through `sectionName`. See more: [Gateway Resource Overview](../ops-guide/gateway/overview.md).

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

- HTTP and stream (TCP/UDP) plugins — see [Filters Overview](../user-guide/http-route/filters/overview.md)
- finer-grained TLS behavior — see [EdgionTls](../ops-guide/gateway/tls/edgion-tls.md)
- gateway-level advanced configuration — see [EdgionGatewayConfig reference](../ops-guide/configuration/edgion-gateway-config.md)

When you see the `🔌 Edgion extension` marker in the docs, that feature is not part of standard Gateway API.

## 6. Recommended learning order

If this is your first time with Edgion, a practical order is:

1. [Your First Gateway](./first-gateway.md)
2. [Operations Guide / Gateway Resource Overview](../ops-guide/gateway/overview.md)
3. [User Guide / HTTPRoute Overview](../user-guide/http-route/overview.md)
4. Then continue into TLS, filters, plugins, or backend configuration depending on your use case

If you are already debugging implementation details or planning to extend the system, switch to:

- [Developer Guide](../dev-guide/README.md)
