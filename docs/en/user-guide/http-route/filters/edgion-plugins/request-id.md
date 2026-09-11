# RequestId Plugin

> **Edgion extension**: RequestId is configured through `EdgionPlugins`.

The gateway always processes W3C `traceparent` and `tracestate`. No plugin is needed to enable propagation. A valid incoming context is preserved; a missing or invalid context starts a new trace with the sampled bit unset. Propagation does not enable span recording or OTLP export.

By default, the gateway preserves a valid incoming `x-trace-id`, or fills it with the W3C trace ID (32 lowercase hexadecimal characters). It forwards this ID upstream and returns it in the response. An existing custom ID can differ from the W3C ID. Neither ID authenticates the caller.

Use the single RequestId plugin to adjust this default or add business correlation IDs. `RequestTraceId` is internal request data, not another plugin or resource. The former TraceContext plugin has been removed.

## Quick start

This example keeps the default `x-trace-id` and adds `x-order-id`. The Gateway and Service named below must already exist in `production`.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: request-ids
  namespace: production
spec:
  requestPlugins:
    - type: RequestId
      config:
        ids:
          - header: x-order-id
            source:
              type: header
              name: x-business-id
            generateWhenMissing: true
            generator:
              type: uuidV4
              prefix: order-
            forwardUpstream: true
            echoResponse: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - api.example.com
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: request-ids
      backendRefs:
        - name: api-service
          port: 8080
```

## Configuration

| Parameter | Type | Default | Description |
|---|---|---|---|
| `defaultId.enabled` | Boolean | Unchanged; initially true | Disable or enable the implicit `x-trace-id` rule |
| `defaultId.forwardUpstream` | Boolean | Unchanged; initially true | Forward the implicit default ID upstream |
| `defaultId.echoResponse` | Boolean | Unchanged; initially true | Return the implicit default ID to the client |
| `ids` | Array | Empty | Additional rules, in declaration order |
| `ids[].header` | String | Required | Destination header, compared case-insensitively |
| `ids[].source` | KeyGet | None | Fallback source: `traceId`, `header` with `name`, or route-only `ctx` with `name` |
| `ids[].generateWhenMissing` | Boolean | true | Generate only when neither target nor source contains a valid ID |
| `ids[].generator.type` | Enum | `uuidV4` | `uuidV4`, `uuidV7`, `randomHex`, `nanoId`, or `md5` |
| `ids[].generator.prefix` | String | Empty | Prefix added only to generated values |
| `ids[].generator.bytes` | Integer | 16 | Entropy bytes for `randomHex` only; 8–64 |
| `ids[].generator.length` | Integer | 21 | Output length for `nanoId` only; 8–128 |
| `ids[].forwardUpstream` | Boolean | true | Forward the managed ID upstream |
| `ids[].echoResponse` | Boolean | true | Return the managed ID in the response |

`md5` hashes fresh random bytes. It is a legacy correlation format, not a cryptographic authentication mechanism. Prefixes are limited to 64 bytes. Values must be 1–256 printable ASCII bytes, without whitespace or commas. Duplicate target fields or invalid values are not accepted as incoming IDs.

Destination names are limited to 128 bytes. Credential, routing, framing, protocol and W3C headers are reserved; sensitive header/context sources are blocked. Each configuration allows at most 16 rules with distinct destination names. A request has at most 16 managed names across attachments, including suppressed names, with at most 4096 active value bytes. Configure fewer custom names when retaining the default.

## Order and multiple gateways

For each rule, the first valid destination value wins. If it is absent, the gateway reads `source`; if that is absent or invalid, `generateWhenMissing` controls generation. Prefixes do not modify inherited or copied values. Forwarding the chosen value allows subsequent gateways to preserve it.

Core defaults and global rules run before normal request validation. Gateway rules run early when TLS metadata or a unique listener owner identifies the Gateway. A shared listener with ambiguous ownership uses core/global IDs until normal routing establishes the owner, then applies Gateway rules before preflight and later plugins. No Gateway is selected arbitrarily for a malformed or unmatched request.

This lifecycle applies to the main HTTP proxy, including gRPC and WebSocket handshakes. The separate HTTP-to-HTTPS redirect service does not run the RequestId plugin chain. Failures before parsed HTTP headers or before a request context exists cannot guarantee response IDs.

Route rules run at their normal request-stage position. They add IDs to the existing state; they do not replace all Gateway rules. An earlier rejection prevents later route rules from running. BackendRef attachments are unsupported. Early rules may only use header/trace-ID sources and header/method/trace-ID key conditions; body, dye, route context and other unavailable predicates are rejected.

To remove the implicit default and use your own ID, set `defaultId.enabled: false` and configure `ids`. An explicit `x-trace-id` rule is allowed. Later disabling the implicit default does not delete an explicitly configured custom rule. Disabling removes the managed default from forwarding, response and correlation logs; an upstream echo cannot restore it. Removal applies to this gateway: another gateway may generate its own default when it sees no header.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: custom-trace-id
  namespace: production
spec:
  requestPlugins:
    - type: RequestId
      config:
        defaultId:
          enabled: false
        ids:
          - header: x-correlation-id
            source:
              type: traceId
            echoResponse: true
```

## Responses and logs

The gateway does not automatically return `traceparent` or `tracestate`. Return the trace ID through `x-trace-id` or a custom rule with `source.type: traceId`. This is a correlation response header, not a W3C response propagation protocol. For browser access across origins, expose the selected header through your [CORS policy](./cors.md).

`echoResponse: false` removes that managed response header, including an upstream echo. `forwardUpstream: false` removes it from the managed upstream request. These flags do not turn off W3C propagation.

Access logs contain `request_info.trace_id.w3c.trace_id` and `request_info.trace_id.correlation_ids`, a JSON object keyed by header name. Copied client/header/context values are masked in logs; propagation and response values are unchanged. Generated values and the canonical W3C ID remain available for correlation. Use the W3C field for trace lookup when a custom value is masked. Exporting logs to OTLP remains a separate configuration choice.

## Related links

- [Access logs](../../../../ops-guide/observability/access-log.md)
- [Request mirror](./request-mirror.md)
- [Filters overview](../README.md)
