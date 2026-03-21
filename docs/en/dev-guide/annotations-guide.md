# Annotations And `edgion.io/*` Extension Keys

This document explains which `edgion.io/*` keys in Edgion are real `metadata.annotations`, which belong to `options` or `labels`, and which are reserved or test-only.

If you are changing code, debugging behavior, or updating docs, start with the agent-facing entry:

- [05-annotations-reference.md](../../../skills/02-features/10-annotations/00-annotations-overview.md)

The detailed key tables are split into these references:

- [annotations-metadata.md](../../../skills/02-features/10-annotations/references/annotations-metadata.md)
- [annotations-options-and-labels.md](../../../skills/02-features/10-annotations/references/annotations-options-and-labels.md)
- [annotations-system-and-test-keys.md](../../../skills/02-features/10-annotations/references/annotations-system-and-test-keys.md)

## Start With Placement, Not Just The Key Name

The same `edgion.io/*` prefix appears in different config locations:

| Location | Example | Meaning |
|----------|---------|---------|
| `metadata.annotations` | `Gateway.metadata.annotations["edgion.io/enable-http2"]` | The most common extension entry point |
| `listener.tls.options` | `listener.tls.options["edgion.io/cert-provider"]` | Listener TLS extension, not an annotation |
| `BackendTLSPolicy.spec.options` | `spec.options["edgion.io/client-certificate-ref"]` | Backend TLS extension, not an annotation |
| `metadata.labels` | `edgion.io/leader` | Scheduling / ownership labels, not annotations |

If the placement is wrong, the code path and the documentation usually drift with it.

## High-Frequency `metadata.annotations`

### Gateway

| Key | Effect |
|-----|--------|
| `edgion.io/enable-http2` | Controls HTTP/2 support |
| `edgion.io/backend-protocol` | Backend protocol extension for TLS listeners; the common current value is `"tcp"` |
| `edgion.io/http-to-https-redirect` | Enables redirect on non-TLS listeners |
| `edgion.io/https-redirect-port` | Target redirect port |
| `edgion.io/metrics-test-key` | Integration-test metrics correlation key |
| `edgion.io/metrics-test-type` | Integration-test metrics mode |
| `edgion.io/edgion-stream-plugins` | Gateway-level connection-filter entry |

### Route / TLS / Backend

| Key | Resource | Effect |
|-----|----------|--------|
| `edgion.io/max-retries` | `HTTPRoute` / `GRPCRoute` | Route-level retry override. It wins over global config, and `0` disables retries. |
| `edgion.io/edgion-stream-plugins` | `TCPRoute` / `TLSRoute` | Resolves an `EdgionStreamPlugins` reference |
| `edgion.io/proxy-protocol` | `TLSRoute` | Current implementation recognizes `"v2"` |
| `edgion.io/upstream-tls` | `TLSRoute` | Controls whether upstream connections use TLS |
| `edgion.io/max-connect-retries` | `TLSRoute` | Max upstream connection attempts |
| `edgion.io/expose-client-cert` | `EdgionTls` | Exposes mTLS client certificate info to the plugin/session layer |
| `edgion.io/health-check` | `Service` / `EndpointSlice` / `Endpoints` | Active health-check YAML config |

## Common Keys That Are Not Annotations

| Key | Actual location | Meaning |
|-----|-----------------|---------|
| `edgion.io/cert-provider` | `Gateway.spec.listeners[*].tls.options` | Listener TLS certificate-provider extension |
| `edgion.io/client-certificate-ref` | `BackendTLSPolicy.spec.options` | Upstream mTLS client certificate reference |
| `edgion.io/leader` | `metadata.labels` | K8s HA leader label |
| `edgion.io/managed-by` | `metadata.labels` | Ownership label for ACME and other system-managed resources |
| `edgion.io/acme-resource` | `metadata.labels` | ACME ownership/resource label |

## Reserved And Test-Only Keys

These keys are often useful during debugging, but should not usually be hand-authored in regular manifests:

| Key | Type | Meaning |
|-----|------|---------|
| `edgion.io/hostname-resolution` | reserved annotation | Diagnostic data written by the controller on `HTTPRoute` / `GRPCRoute` |
| `edgion.io/sync-version` | reserved annotation | Correlates control-plane and data-plane state |
| `edgion.io/skip-load-validation` | test/tooling annotation | Lets the config-load validator skip a YAML file |
| `edgion.io/force-sync` | test annotation | Used by integration scripts to trigger Secret update events |
| `edgion.io/trigger` | operational annotation | Manually re-triggers ACME processing |

## Historical Drift To Watch For

- The current prefix is `edgion.io`, not `edgion.com`.
- The current stream-plugin key for Gateway/TCPRoute/TLSRoute is `edgion.io/edgion-stream-plugins`.
- Older docs and examples previously used `edgion.io/stream-plugins`; do not copy that legacy form into new work.
- `edgion.io/cert-provider` and `edgion.io/client-certificate-ref` look like annotations, but they belong to `options`.

## Related Docs

- [HTTP to HTTPS Redirect](../ops-guide/gateway/http-to-https-redirect.md)
- [Backend Active Health Check](../user-guide/http-route/backends/health-check.md)
- [Stream Plugins User Guide](../user-guide/tcp-route/stream-plugins.md)
- [knowledge-source-map.md](./knowledge-source-map.md)
