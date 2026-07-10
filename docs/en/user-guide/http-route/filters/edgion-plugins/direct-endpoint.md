# Direct Endpoint Plugin

> **🔌 Edgion extension**
> 
> DirectEndpoint is a direct endpoint routing plugin provided by the `EdgionPlugins` CRD, not part of the standard Gateway API.

## Overview

Direct Endpoint allows routing requests directly to a specific endpoint IP via request metadata (Header / Query / Cookie), bypassing load balancing algorithms. The specified endpoint must belong to a Service in the current route's `backendRefs`.

Useful for debugging, targeted testing, and session affinity scenarios.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: direct-endpoint-plugin
spec:
  requestPlugins:
      type: DirectEndpoint
      config:
        from:
          type: header
          name: X-Target-Endpoint
```

---

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `from` | Object | **Required** | Endpoint value source (see sub-fields) |
| `from.type` | String | `header` | Source type: `header` / `query` / `cookie` / `ctx` |
| `from.name` | String | `X-Target-Endpoint` | Source name |
| `extract` | Object | none | Extraction rules |
| `extract.regex` | String | **Required** (within `extract`) | Regular expression applied to the raw value |
| `extract.group` | Integer | `1` | Capture group index (`0` = full match) |
| `port` | Integer | none | Port to use when the value carries no `ip:port` and the backendRef has none |
| `onMissing` | Enum | `Fallback` | Behavior when the value is missing. Allowed values: `Fallback` (normal LB) / `Reject` (400) |
| `onInvalid` | Enum | `Reject` | Behavior when the endpoint is not in any backendRef. Allowed values: `Reject` (403) / `Fallback` |
| `inheritTls` | Boolean | `true` | Inherit TLS from the matched backendRef's BackendTLSPolicy |
| `dyeHeaders` | Object | none | Traffic-tagging headers applied after a successful endpoint resolution. `request`/`response` entry lists of `{name, value}`; an omitted `value` carries the resolved endpoint `ip:port`. See [Scenario 2: Dye Headers](#scenario-2-dye-headers) below. |

---

## Usage Scenarios

### Scenario 1: Specify Target via Header

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-IP
      onMissing: fallback
      onInvalid: reject
```

**Test**:
```bash
curl -H "X-Target-IP: 10.0.1.5" https://api.example.com/debug
```

### Scenario 2: Dye Headers

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-IP
      dyeHeaders:
        response:
          - name: X-Direct-Endpoint
      onMissing: fallback
```

`dyeHeaders` is default-off: when omitted, the plugin sets no headers and strips nothing. When configured, it tags a successfully-resolved request with operator-chosen HTTP headers — `request` entries are set on the request forwarded to the upstream, `response` entries are set on the **response** returned to the client (matching the plugin's traditional `X-Direct-Endpoint` header above, which was always a downstream response header, not a request header). An omitted `value` carries the resolved endpoint `ip:port`; a static `value` is sent as-is:

```bash
curl -H "X-Target-IP: 10.0.1.5" https://api.example.com/debug  # client sees X-Direct-Endpoint: 10.0.1.5:8080 on the response
```

Dyeing happens only after the endpoint successfully resolves against the route's backends — the `onMissing` (value absent) and `onInvalid` (endpoint not in any backendRef) paths never dye, regardless of whether they reject or fall back. Any client-supplied request header matching a configured **request**-direction dye name is stripped before the plugin runs, on every path, so a client can never forge a request-direction tag the backend trusts. Header names go through validation at admission: framing-sensitive names (`Host`, `Content-Length`, `Transfer-Encoding`, `Connection`, `TE`, `Upgrade`, `Trailer`) are rejected, and so is a request-direction dye name equal to the plugin's own `from` header (the anti-spoof strip would remove the endpoint value before extraction).

### Scenario 3: Regex Extraction

```yaml
requestPlugins:
  - type: DirectEndpoint
    config:
      from:
        type: header
        name: X-Target-Info
      extract:
        regex: 'ip=([0-9.]+)'
        group: 1
      port: 8080
```

---

## Behavior Details

- The specified endpoint IP must belong to the Endpoints list of a Service in the current route's `backendRefs`
- If the endpoint doesn't belong to any backend, it is handled as `onInvalid` (default `Reject` with 403)
- The port is resolved in priority order: (1) port embedded in the value (`ip:port`), (2) the `port` field, (3) the matched backendRef's port. If none are available, the plugin returns an error
- `dyeHeaders` is default-off; when configured, it applies the configured request/response headers only after a successful endpoint resolution — see [Scenario 2: Dye Headers](#scenario-2-dye-headers) above
- `Fallback` mode falls back to normal load balancing selection

---

## Troubleshooting

### Problem 1: Specified IP Routes to Different Instance

**Cause**: The specified IP doesn't belong to the current route's backend Service.

**Solution**:
```bash
kubectl get endpoints <service-name> -o yaml
```

### Problem 2: Returns 400 Bad Request

**Cause**: The endpoint value is missing from the request and `onMissing` is set to `Reject`.

**Solution**: Provide the configured source (header/query/cookie/ctx), or set `onMissing: fallback` to use normal load balancing when the value is absent.

### Problem 3: Returns 403 Forbidden

**Cause**: The resolved endpoint is not part of any of the route's `backendRefs` and `onInvalid` is `Reject` (the default).

**Solution**: Confirm the endpoint IP belongs to a backend Service's Endpoints, or set `onInvalid: fallback`.

---

## Related Docs

- [Dynamic Upstream](./dynamic-upstream.md)
- [Backends](../../backends/README.md)
- [Filters Overview](../overview.md)
