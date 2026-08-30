# Design

## Baseline

On ARM64, the compiler reports `EdgionHttpContext` as 2,672 bytes with
8-byte alignment. The largest inline members are:

| Member | Bytes |
|--------|------:|
| `request_info` | 528 |
| `selected_backend` | 504 |
| `selected_grpc_backend` | 224 |
| `backend_context` | 192 |
| `gateway_info` | 176 |

The current default constructor also reserves three `StageLogs` entries. On a
64-bit target this requests 192 bytes from the allocator even when no plugin
produces a log.

## Ownership model

### Common inline state

Keep frequently accessed request identity, timing, retry, body policy/storage,
backend attempt history, and response terminal arbitration inline. These values
are present on most proxied requests and must not gain a mandatory allocation.

### Immutable generation state

Store request-stable `Arc` snapshots or stable `(Arc<Vec<_>>, index)` handles
for Gateway/listener information and compiled backends. Request-specific
effective policy is stored separately from the immutable compiled object.

### Optional scenario state

Allocate at most one state object when a scenario becomes active:

- `PluginRequestState`
- `RequestBodyPluginState`
- `ResponseProcessingState`
- `ParsedRequestParams`
- existing `WafRequestContext`
- existing `AiHttpState`

The helpers that create these states are the only mutation entry points. No
caller directly installs a second cache or side structure.

## Access-log boundary

`AccessLogEntry` borrows compact final facts. It must not trigger parsing,
matcher execution, GeoIP lookup, body materialization, or other lazy work.
Transient runtime state may be released once its final bounded summary is
settled.

WAF receives a dedicated optional top-level block containing source, mode,
outcome, fixed phase slots, bounded findings, first intervention, and first
terminal error. Matched input, headers, cookies, query values, rule messages,
operator parameters, and runtime error strings are forbidden.

## Change order

1. Record layout and allocation baselines.
2. Remove unconditional stage-log allocation.
3. Share Gateway information and compact selected backends.
4. Split common/TLS/gRPC request information.
5. Aggregate plugin-only state.
6. Aggregate request-body-plugin and response-processing state.
7. Compact lazy route parameters and remaining rare state.
8. Add the dedicated WAF access-log block.
9. Run final layout, allocation, functional, and performance verification.

Each step must preserve a buildable commit and receive an independent read-only
subagent review before commit.

## Lifecycle and reset matrix

| State | Lifetime | Retry behavior | Finalization/invalidation |
|-------|----------|----------------|---------------------------|
| Request identity and entry URI snapshot | request | retained | finalized into the terminal access log |
| Normalized current URI/path | request, mutable before URI finalization | retained | a URI rewrite refreshes the normalized path; the entry snapshot never changes |
| Parsed query/path parameters | tied to the documented entry/current URI view | retained unless the owning URI view changes | helper methods own initialization and invalidation; callers never mutate the maps directly |
| Plugin stage scratch | one plugin callback or stage | cleared/restored at the stage boundary | only bounded final stage logs survive |
| Plugin request state | request | retained | dropped with the request after logging |
| Backend attempt state | one upstream attempt | cleared before the next attempt | the completed attempt summary moves into `BackendContext` |
| Response processor attachments | request | retained only when their contract spans attempts | finalized once at transport terminal or logging fallback |
| Response attempt observations | one upstream attempt | reset before retry | must not overwrite committed downstream terminal state |
| WAF transaction and summary | request | follows the explicit WAF retry/body policy | finalized exactly once without creating absent state |

`ResponseProcessingState` may contain sub-structures with different reset
boundaries, but its retry helper must reset only attempt-local members. Grouping
fields behind one pointer does not make their semantic lifetimes identical.

WAF finalization must cover early request rejection, request-body abort,
upstream failure, header-only response, normal body EOS, client disconnect, and
the terminal logging fallback. Access-log construction is read-only and must
never allocate WAF or plugin state merely to serialize an absent block.
