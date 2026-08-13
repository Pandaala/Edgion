---
name: a2a-gateway
description: A2A Gateway ownership, local Agent Card publication, JSON-RPC request observation, route-shape activation, and bounded response observability.
---

# A2A Gateway

`A2aGateway` is a request-stage `EdgionPlugins` entry. It adds two narrowly
gated capabilities on top of the canonical `EdgionBackend.spec.a2a` transport:

1. publish an operator-owned Agent Card from a backendless discovery route;
2. observe the top-level JSON-RPC method on a route whose positive-weight
   backends are typed A2A `EdgionBackend` references.

It does not create an outbound client, resolver, dialer, TLS builder, retry
scheduler, body cache, or backend-selection path.

## Activation is determined by the matched rule

An attached entry is inert unless the already-matched HTTPRoute rule has one of
these shapes:

- **Local Card:** no `backendRefs`; every match explicitly declares `GET` plus
  `PathPrefix` is not accepted—the path must be `Exact` and equal to
  `/.well-known/agent-card.json`. The live request must also be that GET.
- **RPC proxy:** at least one positive-weight backend and every positive-weight
  authorized target is syntactically `edgion.io/EdgionBackend`. A currently
  available runtime entry must be the A2A variant. Missing syntactic
  EdgionBackend references and denied slots remain eligible so the normal
  Gateway API invalid-share behavior stays authoritative. Authorized Service
  references are never reclassified as an eligible missing share. The live
  request must be `POST`.

The Controller applies the same rule-shape checks for route-level ExtensionRefs
and reports incompatible shapes as `Accepted=False`. Runtime shape gating still
protects broader attachment paths. A request-scoped first-writer latch rejects
multiple qualifying `A2aGateway` activations rather than applying two cards or
two body observations.

Qualifying RPC classification pins one variant-neutral EdgionBackend snapshot
in the request context. Canonical upstream selection reuses that exact
generation, preventing a variant reload between protocol gating and selection.
Non-qualifying requests do not pin or alter backend selection.

## Operator-owned local Agent Card

The card is inline JSON in the plugin configuration. Admission validates the
A2A v1 required core shape: `name`, `description`, `version`, `capabilities`,
non-empty `supportedInterfaces`, `defaultInputModes`, `defaultOutputModes`, and
`skills`; every skill requires non-empty `id`, `name`, `description`, and
`tags`. Optional and extension fields are preserved opaquely rather than fully
typed or semantically validated. Compact input is capped at 1 MiB. Legacy
top-level `url`, non-empty `signatures`, invalid retained `JSONRPC`/`1.0`
HTTP(S) URLs, and cards without a `JSONRPC`/`1.0` interface are rejected. Other
bindings retain their protocol-native non-empty URL/address after basic
whitespace/control validation and are filtered from Phase-1 publication.
Unknown fields are retained.

Publication filters `supportedInterfaces` to `JSONRPC`/`1.0`, preserves retained
order and extension fields, and replaces only each URL's origin. Path and query
remain byte-semantically owned by the configured URL. The trusted origin is:

1. the static origin-only `externalUrl`, when configured; otherwise
2. the normalized routed hostname, actual downstream TLS state, and listener
   port.

Forwarded host/proto headers are never trusted. If downstream TLS state is not
known and no `externalUrl` exists, publication fails closed before a response is
committed. Rewritten compact output is independently capped at 1 MiB. Success
is a complete local `200 application/json` response with `Content-Length` and
`Cache-Control: no-store`; no upstream is selected or called.

An Agent Card returned by an A2A backend is ordinary transparent upstream
content in Phase 1. The Gateway neither verifies nor rewrites that response.
Doing so would require a canonical bounded writable response processor and is
not implied by local-card support.

## JSON-RPC method observation

RPC observation first requires exactly one `A2A-Version: 1.0` header and a
canonical unencoded JSON content type. A header-gate failure uses the configured
`failOpen`: `false` returns a generic `400`; `true` forwards unchanged without
reading the body.

After the gate, the request uses the existing request-scoped `BodyStorage` and
its bounded raw scanner. The shared JSON selector extracts only the unique
top-level string `method`; it does not deserialize a full `serde_json::Value`
tree. The method is limited to 64 decoded UTF-8 bytes and control characters
are rejected. Missing, malformed, duplicated, batch, non-string, or oversized
methods produce an unknown observation and do not change the request. Accepted
request bytes are replayed upstream byte-for-byte through the canonical body
path. Body capture/spill/cap failures use the existing body failure policy and
are not softened by `failOpen`.

After capture succeeds, scanner initialization or scan failure is a
best-effort metadata failure: it records only a static unavailable result and
forwards unchanged. It is distinct from capture failure, where no forwarding
copy may exist and canonical fail-close remains mandatory.

The method is sensitive access-log data. It is recorded only through the
existing redaction-aware plugin log; no payload, params, result, task, or raw
body is logged.

## Bounded response observability

A qualifying RPC proxy installs one request-scoped response observer after the
`A2A-Version` and content-type gate succeeds. It is installed even when request
method metadata is unavailable. The observer runs as the read-only tail of the
canonical response-stream driver: it never rewrites, delays, releases,
discards, or retries response bytes and cannot affect the client result.

For a supported `application/json` or concrete `application/*+json` response,
the observer retains at most 1 MiB in its own bounded side accumulator. The limit is inclusive. A streamed response over
the limit, malformed JSON, incomplete EOS, cancellation, or upstream abort
produces only an `unknown` observation; forwarding remains streaming and
unchanged. This side accumulator is not `BodyStorage`, is not exposed to
plugins, and is never an unbounded or reusable response cache.
The process also enforces a 64 MiB atomic reservation budget across concurrent
JSON observers. A request that cannot reserve its bounded slot records a static
unknown outcome and continues forwarding. Reservations are released at the
response terminal boundary rather than waiting for access-log serialization.

For `text/event-stream`, the observer feeds bytes to the single WP7
`SseDecoder`. It does not create another SSE parser or semantic window. Because
observation makes no writable policy decision, it must not instantiate
`RawWindow`: every received byte is already released by the canonical driver,
the observer retains no unreleased prefix, and its observation state cannot
create backpressure. Decoder degradation makes the observation unknown without
changing the stream.

The recognized A2A v1 `StreamResponse` result members are `task`, `message`,
`statusUpdate`, and `artifactUpdate`. Task state is read only from
`result.status.state` for a direct Task result, `result.task.status.state` for
a wrapped task, or `result.statusUpdate.status.state` for a stream update. The bounded
state vocabulary covers `TASK_STATE_UNSPECIFIED`, `SUBMITTED`, `WORKING`,
`COMPLETED`, `FAILED`, `CANCELED`, `INPUT_REQUIRED`, `REJECTED`, and
`AUTH_REQUIRED`; arbitrary values remain unknown. Do not use legacy `kind`,
`final`, or `taskStatusUpdate` fields removed before A2A v1.

At normal EOS or terminal cleanup, the observer appends exactly one static,
bounded summary to the original `A2aGateway` `PluginLog`. The summary may carry
only fixed enums and a saturating event count: outcome class, result/event kind,
bounded error-code class, final task-state class, and event count. It must never
contain JSON-RPC IDs, task/context/artifact IDs, error message/data, message or
artifact content, arbitrary state strings, or raw payload. No per-event logs,
new access-log field, metric, tracing call, or response storage is permitted.

Hot reload affects new requests through the normal immutable plugin snapshot.
An in-flight response keeps its request-pinned observer and summary owner; it is
never migrated to a newer plugin generation.

Related references:

- [HTTP plugin development](12-edgion-plugin-dev.md)
- [EdgionBackend architecture](../05-resources/22-edgion-backend.md)
- [EdgionPlugins schema](../../02-features/03-resources/13-edgion-plugins.md)
