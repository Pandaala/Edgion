# Guardrail Plugin

Guardrail evaluates bounded semantic windows from a canonical streaming AI
response before those bytes are released to the client. For every window it
sends one bounded HTTP request to an operator-managed policy service. The
service chooses exactly one action:

- `pass`: release the original SSE bytes byte-for-byte;
- `replace`: rewrite only the selected semantic JSON string tokens while
  preserving SSE framing and all unrelated event fields;
- `reject`: discard every unreleased byte; replace a held first response with
  one complete 403 response, or terminate the stream after its head has
  already committed.

Guardrail is a `requestPlugins` entry because one matched entry owns the whole
request/response transaction. Do not add a separate response-stage entry.

## Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: reviewed-ai-stream
  namespace: default
spec:
  requestPlugins:
    - type: AiProxy
      config:
        backendSelection:
          type: Weighted
    - type: Guardrail
      config:
        target:
          name: policy-service
          namespace: policy-system
          port: 8080
        timeoutMs: 1000
        maxResponseBytes: 65536
        request:
          path:
            template: /v1/decision
          headers:
            custom:
              - name: Authorization
                template: "Bearer ${secretRef:default/guardrail-token:token}"
        windowBytes: 4096
        overlapBytes: 256
        maxRequestBytes: 1048576
        maxReplacementBytes: 1048576
        maxConcurrent: 64
        holdFirstWindow: true
        headHoldTimeoutMs: 5000
        failOpen: false
```

Attach the resource through an `ExtensionRef` on the AI route. The route must
produce a successful, identity-encoded `text/event-stream` response through
the canonical AI translation/event path. Generic SSE endpoints, non-AI JSON,
WebSockets, and duplex HTTP streams are not Guardrail inputs.

## Configuration

| Field | Default | Limits and behavior |
|-------|---------|---------------------|
| `target` | required | External HTTP origin or Kubernetes Service target. Service targets require `name` and `port`. URL targets may set `blockPrivate`. |
| `tls` | system trust | Shared outbound CA, verification, and mTLS configuration. |
| `timeoutMs` | `5000` | One static deadline covering permit wait, request, response drain, and parsing. `timeoutMsTemplate` is rejected. |
| `maxResponseBytes` | `1024` | Inclusive cap for the complete decision response body. |
| `request.path.template` | required | Restricted outbound path template. |
| `request.headers.custom` | empty | Optional custom headers. `Content-Type` is fixed to `application/json`. Secret templates are resolved by the Controller and redacted in admin output. |
| `windowBytes` | `4096` | Decoded current semantic bytes that trigger a decision, `1..=1048576`. |
| `overlapBytes` | `256` | Prior client-visible context retained per logical bucket, `0..=262144`; a separate 256 KiB global cap also applies. |
| `maxRequestBytes` | `1048576` | Inclusive cap for the serialized decision request, `1..=1048576`. |
| `maxReplacementBytes` | `1048576` | Inclusive aggregate decoded replacement cap, `0..=1048576`. |
| `maxConcurrent` | `64` | Shared in-flight decision limit for this immutable plugin snapshot, `1..=1024`. |
| `holdFirstWindow` | `false` | When enabled, delay the final downstream response head until the first semantic window resolves. This opts the request out of cache lookup/fill and requires `failOpen: false`. |
| `headHoldTimeoutMs` | `5000` | Absolute first-head deadline, including waiting for source bytes, permit acquisition, dependency I/O, and decision parsing, `1..=60000`. Used only when `holdFirstWindow` is enabled. |
| `failOpen` | `false` | On a dependency failure, release the original held bytes and disable Guardrail for the rest of this response. It never changes a policy Reject. |

Guardrail fixes the method to POST. Retry, custom success rules, request body,
query, cookies, forwarded headers, and client-controlled path/header templates
are rejected. A fixed POST is not replayed after an ambiguous network failure.

## Decision protocol

The request is a versioned JSON object. Windows are sorted and kept separate
by choice, semantic field, and tool-call index:

```json
{
  "version": "v1",
  "windows": [
    {
      "choiceIndex": 0,
      "field": { "type": "content" },
      "context": "bounded prior text",
      "current": "text awaiting a decision"
    }
  ]
}
```

Field types are `content`, `reasoningContent`, and
`toolArguments` (which also has `toolCallIndex`). Guardrail does not send raw
SSE, prompts, HTTP headers, model/provider identifiers, or backend addresses.
Configured authentication headers are the only additional outbound exposure.

The service must return a 2xx response with exactly one
`Content-Type: application/json` value and one strict JSON object:

```json
{"version":"v1","action":"pass"}
```

```json
{"version":"v1","action":"replace","replacements":[{"choiceIndex":0,"field":{"type":"content"},"current":"reviewed text"}]}
```

```json
{"version":"v1","action":"reject","code":"policy_code","reason":"Public bounded reason"}
```

A Replace response must name every current logical bucket exactly once and no
other bucket. Unknown, duplicate, missing, trailing, invalid, or oversized
fields are dependency failures. `code` is optional ASCII
`[A-Za-z0-9_.-]` up to 64 bytes; `reason` is optional UTF-8 up to 256 bytes.
Neither value is sent to the downstream client or logged.

A bucket may contain prior `context` but have an empty `current` because it has
no semantic fragment in this window. Its replacement must be empty: there is
no current JSON string token in which to emit additional text.

## Streaming and failure behavior

The gateway allows only one decision in flight per downstream request and
awaits it inline, which supplies backpressure without buffering the full
response. Each immutable configuration snapshot owns one pooled managed HTTP
client and one shared concurrency limit; a hot reload gives new requests a new
snapshot while in-flight requests keep the old one.

Pass releases original bytes unchanged. Replace preserves comments, event/id
fields, usage, finish reasons, line endings, and `[DONE]`; only the current
semantic JSON string ranges change.

The next window's context retains a bounded suffix of the previously accepted
text followed by the text actually emitted by Pass or Replace. A short or empty
replacement therefore does not erase valid earlier context. At EOS, replacement
does not allocate context for a nonexistent next window.

Window memory limits cover simultaneously live visible buffer capacities,
including input metadata, semantic scratch, snapshots, and replacement output.
They are independent of the callout and callback-output limits and are not a
total process RSS limit. A record whose input handle table itself exceeds the
metadata limit is rejected even when its payload is small.

With the default `holdFirstWindow: false`, a valid Reject discards the current
unreleased window and terminates the already-committed stream. With
`holdFirstWindow: true`, the final post-plugin response head and bounded body
prefix remain uncommitted through the first semantic decision. A first-window
Reject replaces them with one complete `403 application/json` response. A
first-window Pass or Replace releases the selected origin head and ordered
processed prefix; later windows continue with normal bounded streaming and can
no longer change the committed status.

Dependency failures include timeout, concurrency saturation, DNS/TLS/egress
failure, response-cap overflow, invalid status/content type, malformed JSON,
and source/bounds failures. With `holdFirstWindow: true`, failures before the
head decision discard the origin prefix and enter the non-retryable 503 error
path; `failOpen: true` is rejected at configuration admission. Without the head
hold, `failOpen: false` terminates the committed stream after held bytes are
discarded, while `failOpen: true` releases the original held bytes and disables
Guardrail for the rest of that response.

## Composition and observability

The response order is canonical AI translation and usage/quota accounting,
then Guardrail, then ordinary writable processors such as ExtProc, then
read-only filters and bandwidth limiting. Guardrail and ExtProc are independent
processors; neither wraps the other.

Access logging records one bounded aggregate Guardrail result for the complete
downstream request. It does not create per-window log entries and never logs
semantic content, replacement text, credentials, the complete target, or raw
dependency errors. When the first-head barrier is enabled, the same line also
records its content-free outcome, aggregate resource usage, configured limits,
and elapsed hold duration. Guardrail callouts are plugin dependency traffic,
not backend attempts or AI provider usage.
