# Your First Gateway

This page gives you the smallest understandable object set:

1. `GatewayClass`
2. `Gateway`
3. `HTTPRoute`

The goal is not to cover every field. It is to help you see how one request path is wired end to end.

## Step 1: GatewayClass

`GatewayClass` decides which controller manages the gateway.

Minimal example:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: public-gateway
spec:
  controllerName: edgion.io/gateway-controller
```

If you need Edgion-specific gateway-level configuration, you can later add `parametersRef`. See:

- [Operations Guide / GatewayClass Configuration](../ops-guide/gateway/gateway-class.md)

## Step 2: Gateway

`Gateway` defines the traffic entry points, which means listeners.

Minimal HTTP example:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: internal-gateway
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http
      protocol: HTTP
      port: 80
```

This step establishes:

- which `GatewayClass` is used
- which protocol and port are exposed
- which listener later routes will bind to

More listener details:

- [Operations Guide / Gateway Resource Overview](../ops-guide/gateway/overview.md)
- [Operations Guide / HTTP Listener](../ops-guide/gateway/listeners/http.md)

## Step 3: HTTPRoute

`HTTPRoute` binds requests to the gateway listener and forwards traffic to backend services.

Minimal example:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: edgion-test
spec:
  parentRefs:
    - name: internal-gateway
      sectionName: http
  hostnames:
    - "example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: echo-service
          port: 8080
```

The most important pieces are:

- `parentRefs` binds the route to the `Gateway`
- `sectionName` matches the listener name
- `backendRefs` points to the actual backend service

More details:

- [User Guide / HTTPRoute Overview](../user-guide/http-route/overview.md)
- [User Guide / Service Reference](../user-guide/http-route/backends/service-ref.md)

## Step 4: Understand the traffic path

Once these three objects are connected, the traffic path is roughly:

1. the client sends traffic to the `Gateway` listener
2. `HTTPRoute` matches by hostname, path, headers, and related rules
3. the selected `backendRefs` are resolved
4. the gateway forwards traffic to the backend service

From there, you usually keep building on `HTTPRoute` with:

- match rules
- standard Gateway API filters
- Edgion extension plugins
- retries, timeouts, and session persistence

## Suggested next steps

If you want to keep learning as a user:

- [User Guide / HTTPRoute Overview](../user-guide/http-route/overview.md)
- [User Guide / Filters Overview](../user-guide/http-route/filters/overview.md)
- [User Guide / Backend Configuration](../user-guide/http-route/backends/README.md)

If you still want the object model first:

- [Core Concepts](./core-concepts.md)
