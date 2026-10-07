# GrpcJsonTranscoder

Attach this request plugin through an HTTPRoute **rule** ExtensionRef. It maps
REST/JSON over HTTP/1.1 or HTTP/2 to native gRPC over mandatory upstream HTTP/2.
The selected HTTPRoute keeps its backend and authorization scope. The upstream
Service does not need an appProtocol hint; an HTTP/1-only origin fails with 502.
TLS, backend policies, load balancing and approved destination jumps retain their
normal ownership. Global, Gateway, backendRef and GRPCRoute attachments are rejected.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rest-grpc
  namespace: default
spec:
  requestPlugins:
    - type: GrpcJsonTranscoder
      body:
        maxBodySize: 1m
        onReadFailure: failClose
      config:
        descriptor:
          type: configMapRef
          name: rpc-descriptor
          key: service.pb
        services:
          - example.Greeter
        autoMapping: false
        streamFormat: ndjson
        maxRequestMessageSize: 1m
        maxResponseMessageSize: 512k
        maxResponseJsonSize: 1m
        maxMessageDepth: 32
        maxMessageNodes: 16384
```

Attach the plugin to an HTTPRoute rule targeting your gRPC Service:

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: rest-grpc
  namespace: default
spec:
  parentRefs:
    - name: gateway
  hostnames:
    - api.example.com
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: rest-grpc
      backendRefs:
        - name: greeter
          port: 50051
```


Compile a FileDescriptorSet with `protoc --include_imports --descriptor_set_out=service.pb`.
Preserve google.api.http options and their imported descriptors. Store the bytes
in same-namespace ConfigMap `binaryData`, or set `descriptor: {type: inline,
base64: <encoded bytes>}`. Decoded descriptors cannot exceed 512 KiB. Updates and
deletions recompile dependent plugins; invalid generations fail closed, including
resources that also contain DSL/WAF. In-flight requests pin their existing generation.
There is no descriptor URL/file watcher, upload service, reflection or runtime protoc.

HTTP annotations support GET/PUT/POST/DELETE/PATCH, path variables, nested fields,
additional bindings and body `""`, `"*"` or a field selector. With autoMapping,
unannotated methods use `POST /package.Service/Method` and body `*`. Unknown query
fields fail with 400 by default; `ignoreUnknownQueryParameters: true` ignores them.
Known overlapping path/query/body fields and duplicate parameters remain invalid.
ProtoJSON retains protobuf int64 strings, enum names, bytes base64, maps and supported
well-known types. Any, HttpBody, groups, custom HTTP methods, response_body,
client/bidirectional streaming and compressed messages are unsupported.

Request-body capture shares the Gateway's canonical BodyStorage and physical cap.
The entry logical cap can tighten the 1 MiB JSON maximum. Place conversion after
policies needing the REST input; later method/path/query/body or protected RPC header
changes fail closed. An existing consumer that freezes the body can reject conversion.
AI conversion owners, native gRPC, gRPC-web and WebSocket requests cannot share it.

Unary success is one ProtoJSON document, committed only after the actual gRPC status.
A malformed/truncated/oversized response or missing status returns a fixed JSON 502.
Actual gRPC errors map to REST statuses without exposing raw grpc-message. The bounded
Unary Hold replacement currently prevents reuse of that upstream connection.

Server streaming emits one record per message and one explicit terminal:

```text
{"type":"message","data":{"value":"example"}}
{"type":"end"}
```

An error terminal uses `type: error` and, when available, numeric `grpcStatus`.
With `streamFormat: sse`, records use `rpc_message`, `rpc_end` and `rpc_error` events.
EOF without a terminal is incomplete. After headers commit, failures are stream
errors or an aborted exchange; the HTTP status is not rewritten.

Hard bounds: 512 KiB per response protobuf message, 1 MiB per JSON record and
outstanding converted output, 256 messages per processing callback, depth 32 and
16384 structural nodes. JSON record limits must be at least 128 bytes. A process-wide
64 MiB budget reserves 32 MiB per active conversion, allowing two concurrent exchanges;
admission failure returns 503. Cancellation and final output release return capacity.
Unary has an absolute Hold timeout of at most 30 seconds. Streaming checks expiration
at callbacks and caps upstream I/O timeouts at 30 seconds; waiting for a transport event
or downstream write can exceed the callback expiration target.

Runtime protobuf message extension ranges are rejected. Nonempty explicit string/bytes defaults are rejected before reflection constructs the descriptor pool, including custom option declarations. Map entries and recognized well-known types must have canonical wire shapes. Transport grpc-status is pinned before mutable trailer companions; companions cannot manufacture success. Immediate streaming output-credit exhaustion cancels the origin through the canonical sink and leaves an incomplete stream if no bounded error terminal can be admitted.

Before reflection, descriptor admission limits packages to 256 bytes and 16 segments,
identifiers to 128 ASCII bytes, and qualified names to 512 bytes. It also bounds
file/import names to 4096 bytes, files to 128, messages to 1024 and fields to 8192.
Descriptor wire structure is independently limited to depth 32 and 16384 nodes.
Oversized or invalid descriptors fail configuration validation.

Cold descriptor admission supports standard protobuf options and resolved
`google.api.http` annotations. Other encoded custom options, uninterpreted options,
relative extension extendees and unknown descriptor fields are rejected. A trusted
schema admits HTTP option payloads under the same depth/node caps before reflection;
raw reserved message/extension checks prevent schema shadowing. Mutable streamed
request-body handlers (including ExtProc STREAMED) cannot coexist with the RPC owner
in either plugin order, including when the processor's failure policy allows errors.

Unary conversion rejects response-body consumers whose verdicts would be bypassed
by final replacement: ExtProc with an effective STREAMED response body, BandwidthLimit,
response-body Wasm, and other installed streaming response processors. This applies
across global, Gateway, rule and selected backendRef scopes and both plugin orders;
fail-open does not override ownership conflicts. Header/trailer-only companions remain
supported. Selected backendRef AI owners are rejected before their plugins and the
origin run. A later protected mutation, including an ignored DSL error in a response
header stage, cannot publish a prepared unary replacement. WAF's intentional empty
response inspection window is unchanged.

The final access log retains one aggregate conversion outcome and, when validated,
`request_info.grpc_status`. Stream failures remain visible even after HTTP 200 has
committed. No payload or upstream grpc-message is recorded.

ProtoJSON null leaves ordinary fields unset, including optional/oneof fields and whole
collections. Value/NullValue sentinels retain their null representation. Unknown fields
and duplicate proto/JSON aliases remain errors even when null. Quoted integral decimal
and exponent forms retain exact 64-bit values; fractional or out-of-range integer
values and finite float spellings that overflow are rejected. Explicit NaN/Infinity/
-Infinity use canonical strings, including wrappers inside lists and maps.

Same-method path templates must be disjoint, including literal/wildcard intersections
and optional multi-segment tails. Publication rejects overlap and bounded-analysis
exhaustion instead of inventing precedence. Different HTTP methods remain independent.
