# A2A Gateway

`A2aGateway` adds A2A v1 discovery and lightweight JSON-RPC request
observation to an HTTPRoute that uses `EdgionBackend.spec.a2a`.

## Publish a local Agent Card

The card is owned by the operator and stored inline. This route must have no
backend and must match only the exact discovery GET:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: agent-card
spec:
  requestPlugins:
    - type: A2aGateway
      config:
        externalUrl: https://agents.example.com
        card:
          name: Example agent
          description: Example A2A agent
          version: 1.0.0
          capabilities: {}
          supportedInterfaces:
            - url: https://internal.invalid/rpc?tenant=blue
              protocolBinding: JSONRPC
              protocolVersion: "1.0"
          defaultInputModes: [application/json]
          defaultOutputModes: [application/json]
          skills:
            - id: echo
              name: Echo
              description: Echo a message
              tags: [example]
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: agent-card
spec:
  parentRefs: [{name: public}]
  hostnames: [agents.example.com]
  rules:
    - matches:
        - method: GET
          path: {type: Exact, value: /.well-known/agent-card.json}
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: agent-card
```

The Gateway retains only `JSONRPC`/`1.0` interfaces and replaces the origin of
their URLs. It preserves the configured path, query, interface order, and
unknown extension fields. `externalUrl` is recommended when the routed host,
scheme, or port is not the public origin. It must be an HTTP(S) origin with no
path or query.

Input and rewritten output are each limited to 1 MiB. Signed cards are not
accepted in this phase. A successful response is `application/json` with
`Cache-Control: no-store`; no backend is called.

## Observe JSON-RPC requests

Attach the plugin to a rule whose positive-weight backends are A2A
`EdgionBackend` references. Valid A2A requests use:

```text
POST /rpc
Content-Type: application/json
A2A-Version: 1.0
```

The Gateway reads the request through its shared request body store, extracts
only a unique top-level string `method` with a bounded scanner, and forwards
the original bytes unchanged. It does not deserialize the full JSON document.
Malformed, missing, duplicated, non-string, or oversized methods are treated
as unknown observations; they are not payload rewrites.

`failOpen: false` returns `400` when the A2A version or JSON media-type gate is
not satisfied. `failOpen: true` forwards such a request without observing its
body. Request body size, spill, and storage failures always use the gateway's
normal request-body policy.

After capture succeeds, a method-scanner failure is best-effort and the
unchanged request continues with an unavailable observation. A qualifying
request also keeps one backend snapshot from protocol gating through upstream
selection, so a backend variant reload cannot change its classification
mid-request.

## Current boundary

An Agent Card returned by a backend is proxied transparently; it is not
verified or rewritten. Response result/error, task state, and SSE event
observation are also outside this phase. The main upstream response remains
streaming and is never placed in a response body store.
