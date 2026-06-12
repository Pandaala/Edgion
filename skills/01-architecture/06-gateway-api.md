---
name: gateway-api-conformance
description: Gateway API v1.5.0 conformance — supported resources, Edgion extension CRDs, conformance testing, extension points.
---

# Gateway API Conformance

Edgion implements the Kubernetes Gateway API v1.5.0 specification, supporting HTTP, gRPC, TCP, TLS, and UDP routing protocols. On top of standard Gateway API, Edgion provides extension functionality through custom CRDs and annotations.

## Supported Gateway API Resources

| Resource | API Group | Channel | Description |
|------|-----------|---------|------|
| **GatewayClass** | gateway.networking.k8s.io/v1 | Standard | Defines the Gateway implementation class; Edgion's controllerName is `edgion.io/gateway-controller` |
| **Gateway** | gateway.networking.k8s.io/v1 | Standard | Defines listeners, ports, and TLS configuration; one Gateway corresponds to one logical entry point |
| **HTTPRoute** | gateway.networking.k8s.io/v1 | Standard | HTTP routing rules: hostname matching, path matching, header matching, request redirect/rewrite/mirror |
| **GRPCRoute** | gateway.networking.k8s.io/v1 | Standard | gRPC routing rules: Service/Method matching, header matching |
| **TCPRoute** | gateway.networking.k8s.io/v1alpha2 | Experimental | TCP layer-4 routing, forwarded by port to backends |
| **TLSRoute** | gateway.networking.k8s.io/v1 (v1alpha3 compatible) | Standard / Experimental — served versions may differ between install manifests | TLS passthrough routing, forwards encrypted traffic directly after SNI matching |
| **UDPRoute** | gateway.networking.k8s.io/v1alpha2 | Experimental | UDP routing, forwarded by port |
| **ReferenceGrant** | gateway.networking.k8s.io/v1 (v1beta1 compatible) | Standard | Cross-namespace reference authorization, used only on the Controller side (no_sync_kinds) |
| **BackendTLSPolicy** | gateway.networking.k8s.io/v1 (v1alpha3 compatible) | Standard / Experimental — served versions may differ between install manifests | Backend TLS policy, configures Gateway→Backend mTLS |

## CRD Installation

Standard Gateway API CRDs are installed via the official YAML:

```bash
# Install Standard channel CRDs
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.5.0/standard-install.yaml

# Install Experimental channel CRDs (includes TCPRoute, UDPRoute, and older served versions of some resources)
kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.5.0/experimental-install.yaml
```

Edgion extension CRDs are installed via Edgion's own deployment manifest.

## Edgion Extension CRDs

| Resource | API Group | Description |
|------|-----------|------|
| **EdgionGatewayConfig** | edgion.io/v1alpha1 | Global / per-Gateway configuration: timeouts, buffer sizes, log levels, global plugins |
| **EdgionPlugins** | edgion.io/v1alpha1 | HTTP plugin collection: referenceable via the ExtensionRef of HTTPRoute/GRPCRoute |
| **EdgionStreamPlugins** | edgion.io/v1alpha1 | TCP/UDP stream plugin collection: connection-level filters (IP allow/deny lists, etc.) |
| **EdgionTls** | edgion.io/v1alpha1 | TLS certificate and key resource: an alternative to standard Secret, supports file path references and inline certificates |
| **PluginMetaData** | edgion.io/v1alpha1 | Plugin metadata: describes plugin capabilities, parameter schema, and version information |
| **LinkSys** | edgion.io/v1alpha1 | External system integration: connection configuration for ES, Redis, Etcd, Webhook, File, and other data sources |
| **EdgionAcme** | edgion.io/v1alpha1 | ACME automatic certificate issuance configuration: Let's Encrypt integration, executed only on the Leader Controller |

## Conformance Testing

Edgion uses the official Go test suite from `sigs.k8s.io/gateway-api/conformance` v1.5.1 for conformance testing.

### How to Run

```bash
# Enter the conformance test directory
cd edgion-tests/conformance

# Run the conformance tests (requires a K8s cluster with Edgion deployed)
go test -v -run TestConformance ./...
```

### Test Coverage

Conformance tests verify the following core behaviors:
- Creation and status updates of GatewayClass and Gateway
- HTTPRoute route matching (hostname, path, Header)
- HTTPRoute filters (RequestRedirect, RequestHeaderModifier, URLRewrite)
- GRPCRoute route matching (Service, Method)
- Cross-namespace references (ReferenceGrant)
- TLS termination and TLS passthrough
- BackendTLSPolicy backend mTLS

## Multi-version Compatibility Notes

Within a single Gateway API release, `standard-install.yaml` and `experimental-install.yaml` may expose different served versions for the same resource.

Practical recommendations:

- The controller should prefer to watch the latest stable version
- Older versions serve only as fallback input versions
- When relying on the API Server's multi-version CRD conversion, do not start watchers on multiple versions of the same resource by default

## Extension Points

Edgion extends standard Gateway API through the following mechanisms:

### 1. Annotations

Use annotations with the `edgion.io/*` prefix to add Edgion-specific behavior to standard resources.
Most `edgion.io/*` annotations tune runtime behavior (e.g. `edgion.io/enable-http2`,
`edgion.io/http-to-https-redirect`, `edgion.io/health-check`, `edgion.io/circuit-breaker`,
`edgion.io/proxy-protocol`). The one annotation that references another resource is:

| Annotation | Applicable Resources | Description |
|------|---------|------|
| `edgion.io/edgion-stream-plugins` | Gateway | References an EdgionStreamPlugins resource by name |

EdgionPlugins, EdgionTls, and EdgionGatewayConfig are **not** wired via annotation keys — they are
associated through `ExtensionRef`-type filters (EdgionPlugins) or spec references (EdgionTls,
EdgionGatewayConfig); LinkSys is its own resource. See the ExtensionRef section below.

For the full list of annotations, see `skills/02-features/05-annotations/00-annotations-overview.md`.

### 2. ExtensionRef Filter

HTTPRoute and GRPCRoute support referencing Edgion extension resources via `ExtensionRef`-type filters:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-plugins
```

### 3. Custom CRDs

All resources listed in the Edgion extension CRDs table above are extensions beyond standard Gateway API. They are associated with standard resources via annotations or ExtensionRef, following the extensibility design principles of Gateway API.

## Gateway API Compatibility Notes

For more detailed compatibility notes, intentional deviations, and known limitations, see `skills/08-kubernetes/01-gateway-api/SKILL.md`.
