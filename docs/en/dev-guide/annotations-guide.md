# Annotations And `edgion.io/*` Extension Keys

This document explains which `edgion.io/*` keys in Edgion are real `metadata.annotations`, which belong to `options` or `labels`, and which are reserved or test-only.

If you are changing code, debugging behavior, or updating docs, start with the agent-facing entry:

- [05-annotations-reference.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/00-annotations-overview.md)

The detailed key tables are split into these references:

- [annotations-metadata.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-metadata.md)
- [annotations-options-and-labels.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-options-and-labels.md)
- [annotations-system-and-test-keys.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-system-and-test-keys.md)

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
| `edgion.io/http-to-https-redirect` | Enables redirect on non-TLS listeners |
| `edgion.io/https-redirect-port` | Target redirect port |
| `edgion.io/metrics-test-key` | Integration-test metrics correlation key |
| `edgion.io/metrics-test-type` | Integration-test metrics mode |
| `edgion.io/edgion-stream-plugins` | Gateway-level connection-filter entry |
| `edgion.io/accept-proxy-protocol` | Expect an inbound PROXY protocol header (v1/v2 auto-detected) on every listener of this Gateway, as sent by an L4 load balancer such as AWS NLB. The addresses it carries replace the peer address the whole request path sees. Connections without a valid header are rejected. **Inbound only** — unrelated to `edgion.io/proxy-protocol` below, which *sends* a header upstream |
| `edgion.io/accept-proxy-protocol.<listener-name>` | Overrides the Gateway-wide value for one listener (a Gateway API `Listener` has no annotations of its own) |
| `edgion.io/proxy-protocol-trusted-cidrs` | Restricts the above to connections from these IP/CIDRs (comma- or space-separated; a bare address means /32 or /128). Peers outside the list are served as ordinary direct connections and their bytes are never parsed as a header, so one listener can serve both load-balanced and direct traffic. Absent means every connection must carry a header. An invalid or match-all entry falls back to requiring a header. Gateway-wide only — no per-listener suffix |

> **Trust:** a PROXY protocol header is unauthenticated. Without
> `proxy-protocol-trusted-cidrs` the listener must be reachable exclusively
> through the trusted load balancer, enforced at the network layer, and must not
> also serve kubelet probes (those hit the pod directly and carry no header).
> With the CIDR list set, other peers are simply served as direct connections.
>
> In both modes the listener must only be reachable via something that writes
> the header **itself**: a transparent L4 relay forwards the client's first bytes
> verbatim, so a client can author a header and the relay launders it into a
> trusted position.
>
> Note also that `edgion.io/edgion-stream-plugins` runs before the header is
> parsed, so its IP rules match the load balancer, not the client.
> See `tasks/proxy-protocol-inbound.md`.

### Route / TLS / Backend

| Key | Resource | Effect |
|-----|----------|--------|
| `edgion.io/edgion-stream-plugins` | `TCPRoute` / `TLSRoute` | Resolves an `EdgionStreamPlugins` reference |
| `edgion.io/proxy-protocol` | `TLSRoute` | Current implementation recognizes `"v2"` |
| `edgion.io/max-connect-retries` | `TLSRoute` | Max upstream connection attempts; default 2, capped at 5 |
| `edgion.io/tcp-keepalive-time` | `TCPRoute` / `TLSRoute` | Idle seconds before the first TCP keepalive probe on the upstream L4 connection. Presence (positive integer, 1–7200) enables keepalive; absent = disabled (default) |
| `edgion.io/tcp-keepalive-interval` | `TCPRoute` / `TLSRoute` | Seconds between keepalive probes (1–75). Only applied when `tcp-keepalive-time` is set; absent or invalid = 75 (OS default) |
| `edgion.io/tcp-keepalive-probes` | `TCPRoute` / `TLSRoute` | Number of failed probes before the connection is declared dead (1–20). Only applied when `tcp-keepalive-time` is set; absent or invalid = 9 (OS default) |
| `edgion.io/expose-client-cert` | `EdgionTls` | Exposes mTLS client certificate info to the plugin/session layer |

Service-level backend behavior (active health checks, outlier detection,
panic threshold, load-balancing algorithm) is **not** an annotation — it is
configured via the `EdgionBackendTrafficPolicy` CRD, attached to a Service via
`spec.targetRefs`. See
[`skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md)
for the full schema, or the user guides:
[Active Health Check](../user-guide/http-route/backends/health-check.md),
[Circuit Breaker](../user-guide/http-route/backends/circuit-breaker.md),
[Load Balancing Algorithms](../user-guide/http-route/lb-algorithms.md).

## Common Keys That Are Not Annotations

| Key | Actual location | Meaning |
|-----|-----------------|---------|
| `edgion.io/cert-provider` | `Gateway.spec.listeners[*].tls.options` | Listener TLS certificate-provider extension |
| `edgion.io/client-certificate-ref` | `BackendTLSPolicy.spec.options` | Upstream mTLS client certificate reference |
| `edgion.io/leader` | `metadata.labels` | K8s HA leader label |
| `edgion.io/managed-by` | `metadata.labels` | Provenance marker on ACME and other system-managed resources; not read by the system |
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
