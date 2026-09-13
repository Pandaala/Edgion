# TLSRoute Overview

TLSRoute matches inbound TLS connections by the SNI hostname in the ClientHello and forwards the encrypted bytes to a backend, leaving TLS termination to the backend itself.

TLSRoute is a standard Gateway API resource. It is the encryption-aware counterpart of TCPRoute: where TCPRoute routes a raw TCP stream by listener port only, TLSRoute peeks at the unencrypted SNI field of the TLS handshake and routes by hostname, letting many hostnames share one listener port.

**TLS passthrough** means the gateway does not decrypt traffic. It reads only the SNI to pick a backend, then proxies the original encrypted stream end to end. This requires the parent Gateway listener to use TLS mode `Passthrough`. This contrasts with **TLS termination**, where the gateway decrypts traffic and works with the plaintext (see [TLS Termination](../../ops-guide/gateway/tls/tls-termination.md)).

## Basic structure

A minimal TLSRoute attaches to a TLS listener, matches one SNI hostname, and forwards to a single backend.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      # Name of the listener with tls.mode set to Passthrough
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
  rules:
    - backendRefs:
        - name: api-service
          port: 443
```

Key fields:

- `parentRefs` — attaches the route to a Gateway listener whose TLS mode is `Passthrough`.
- `hostnames` — the SNI values this route matches.
- `rules[].backendRefs` — the backend(s) that receive the forwarded TLS stream.

## Core concepts

### ParentRefs

`parentRefs` binds the TLSRoute to a Gateway listener. Use `sectionName` to target a specific listener by name; this listener must have `tls.mode: Passthrough` so the gateway forwards encrypted bytes instead of decrypting them. A single TLSRoute may attach to more than one Gateway.

More on listeners: [TLS Termination](../../ops-guide/gateway/tls/tls-termination.md).

### Hostnames (SNI)

`hostnames` is required and must contain 1..16 lowercase DNS names, including short names such as `tls`. Exact names (`secure.example.com`) and wildcard suffixes (`*.internal.example.com`) are supported. A route wildcard matches one or more labels, including `a.b.internal.example.com`, but not the suffix alone. This routing rule is distinct from unchanged certificate selection and certificate wildcard verification. Missing or empty lists, IP literals, malformed names and a bare `*` are rejected; one invalid entry rejects the whole list.

Each attachment uses the intersection of the route's names and that listener's hostname. For example, a route declaring `a.example.com` and `b.example.com` attached to listeners for each name on different ports serves only the corresponding name on each port. A listener without a hostname preserves the route's explicit names; it does not make the route match arbitrary SNI. An empty intersection produces no route on that listener. Changing or removing an attachment replaces its old names without changing unrelated listener snapshots.

Matching declared names are considered from exact to longest wildcard suffix; Listener intersection does not promote a wildcard route's priority. A candidate must match the SNI and ownership checks through the same accepted attachment. Ineligible candidates may yield to another eligible route, but cannot bypass a more-specific Listener. Once a route is selected, policy or backend failure does not select another route.

### Rules

`rules` is the list of routing rules. Unlike HTTPRoute, a TLSRoute rule has no match conditions beyond the SNI selection already done by `hostnames` — the rule exists to carry `backendRefs`. Most TLSRoutes have a single rule.

### BackendRefs

`rules[].backendRefs` lists the destinations for matched connections. Each entry names a Service and a port; when several backends are listed, traffic is split by `weight`. Backends in another namespace require a ReferenceGrant.

More on backends: [Backend configuration](./backends/overview.md).

## Complete example

This TLSRoute matches two exact SNI hostnames plus a wildcard, and splits traffic across two weighted backends.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: tls-passthrough
  hostnames:
    - "secure.example.com"
    - "api.example.com"
    - "*.internal.example.com"
  rules:
    - backendRefs:
        # 90% of matched connections to the stable backend
        - name: api-service
          port: 443
          weight: 90
        # 10% to the canary backend
        - name: api-service-canary
          port: 443
          weight: 10
```

## Subdocument navigation

| Topic | Document | Description |
|-------|----------|-------------|
| Backend configuration | [backends/](./backends/overview.md) | Define target Services, ports, weights, and cross-namespace references |

## Related documents

- [TLS Termination](../../ops-guide/gateway/tls/tls-termination.md)
- [Backend configuration](./backends/overview.md)
- [TCPRoute Overview](../tcp-route/overview.md)
