# Your First Gateway

This page takes you all the way from a running Edgion to a working HTTP request, using the smallest understandable object set:

1. `GatewayClass`
2. `Gateway`
3. `HTTPRoute`

The goal is not to cover every field. It is to wire one request path end to end and verify it with `curl`.

This walkthrough assumes **Kubernetes mode** with a backend `Service` that has running pods. If you are on standalone mode, see [Installation and Deployment](./installation-and-deployment.md) for how the same resources are placed in the file-system config directory.

## Step 0: Install Edgion

Deploy the controller, gateway, CRDs, and base configuration:

```bash
cicd/deploy/kubernetes/scripts/deploy.sh -y
```

Confirm the control plane and data plane are running:

```bash
kubectl -n edgion-system get pods
```

This step establishes a running Controller (which reads resources) and Gateway (which serves traffic). See more: [Installation and Deployment](./installation-and-deployment.md).

## Step 1: GatewayClass

`GatewayClass` decides which controller manages the gateway.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: public-gateway
spec:
  controllerName: edgion.io/gateway-controller/edgion-system/edgion-controller
```

> `GatewayClass` is a cluster-scoped resource and does not have a `namespace` field.

This step establishes:

- which controller (`controllerName`) manages the gateways created later
- the value later `Gateway` resources reference via `gatewayClassName`

The `deploy.sh` base config already creates a `public-gateway` GatewayClass, so you can skip this if you used Step 0. To add Edgion-specific gateway-level configuration later, attach `parametersRef`. See more: [GatewayClass Configuration](../ops-guide/gateway/gateway-class.md).

## Step 2: Gateway

`Gateway` defines the traffic entry points, which means listeners.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: edgion-test
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

This step establishes:

- which `GatewayClass` is used
- which protocol and port are exposed
- which namespaces are allowed to attach routes to this listener

See more: [Gateway Resource Overview](../ops-guide/gateway/overview.md), [HTTP Listener](../ops-guide/gateway/listeners/http.md).

## Step 3: Backend Service

Point the route at a real backend. Any HTTP `Service` works; this example deploys a small echo server:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: echo
  namespace: edgion-test
spec:
  replicas: 1
  selector:
    matchLabels:
      app: echo
  template:
    metadata:
      labels:
        app: echo
    spec:
      containers:
        - name: echo
          image: ealen/echo-server:latest
          ports:
            - containerPort: 80
---
apiVersion: v1
kind: Service
metadata:
  name: echo-service
  namespace: edgion-test
spec:
  selector:
    app: echo
  ports:
    - port: 8080
      targetPort: 80
```

This step establishes a backend the gateway can forward to. See more: [Service Reference](../user-guide/http-route/backends/service-ref.md).

## Step 4: HTTPRoute

`HTTPRoute` binds requests to the gateway listener and forwards traffic to the backend.

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: edgion-test
spec:
  parentRefs:
    - name: main-gateway
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
- `sectionName` matches the listener name (`http`)
- `backendRefs` points to the backend `Service` and port

See more: [HTTPRoute Overview](../user-guide/http-route/overview.md).

## Step 5: Apply and verify

Apply everything (assuming you saved the YAML above to files, or apply them inline):

```bash
kubectl create namespace edgion-test --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f gateway.yaml -f backend.yaml -f httproute.yaml
```

Check that the route attached and the gateway is programmed:

```bash
kubectl -n edgion-test get gateway main-gateway
kubectl -n edgion-test get httproute example-route
```

Send a request through the gateway. For a quick local check, port-forward the gateway Service to your machine. Find the Service name first (it lives in the `edgion-system` namespace):

```bash
kubectl -n edgion-system get svc
kubectl -n edgion-system port-forward svc/<gateway-service> 8080:80
```

In another terminal, send a request with the matching `Host` header:

```bash
curl -H "Host: example.com" http://127.0.0.1:8080/
```

You should see the echo server's response, which confirms the request flowed through the `Gateway`, matched the `HTTPRoute`, and reached `echo-service`.

## Step 6: Understand the traffic path

Once these objects are connected, the traffic path is:

1. the client sends traffic to the `Gateway` listener
2. `HTTPRoute` matches by hostname, path, headers, and related rules
3. the selected `backendRefs` are resolved
4. the gateway forwards traffic to the backend service

From there, you usually keep building on `HTTPRoute` with:

- match rules
- standard Gateway API filters
- Edgion extension plugins
- retries, timeouts, and session persistence

## Next steps

If you want to keep going as a user:

- [HTTPRoute Overview](../user-guide/http-route/overview.md)
- [Filters Overview](../user-guide/http-route/filters/overview.md)
- [Backend Configuration](../user-guide/http-route/backends/README.md)

If you still want the object model first:

- [Core Concepts](./core-concepts.md)
