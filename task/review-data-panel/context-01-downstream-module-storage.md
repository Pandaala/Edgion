# Downstream HTTP module context storage

Status: deferred design decision. Edgion baseline: `a822ba509`.
Inspected Pingora fork revision: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence

`pg_new_ctx` captures one resolved Gateway configuration and constructs
EdgionHttpContext with absent optional owners, empty containers and lazy parsed
request caches. The ordinary constructor's allocation test excludes startup
configuration initialization. Its zero-allocation result does not describe the
separate Pingora Session or its module contexts.

Edgion's `pg_init_downstream_modules` registers ResponseCompressionBuilder and
GrpcWeb. In the pinned fork, Session construction calls HttpModules::build_ctx.
That function collects per-builder state into a Vec; the two builders each
return a Box containing a nonzero-sized mutable request context. The TypeId
lookup map is initialized once and shared by Arc, not rebuilt for every request.
Registration and sorting of builders are also separate from request construction.

GrpcWebBridge starts Disabled and compression initially has an empty
Accept-Encoding Vec without an encoder. These lazy internals do not eliminate
the outer Box/Vec owners. The finding is based on source ownership, not a fresh
allocation count or throughput benchmark of Session construction.

## Decision alternatives

Retain the generic module storage, or authorize a separately scoped Pingora-fork
investigation of inline/small storage or lazy materialization. The latter needs
an explicit external-repository scope and its own IMPL design/reviews; no fork
checkout or dependency revision is changed by this task. Larger inline storage
may trade allocations for per-request size, moves and cache pressure. Lazy
storage may alter typed lookup or callback dispatch and adds branch/ownership
complexity. Neither is automatically faster.

Do not remove GrpcWeb registration when frontend HTTP/2 is disabled: gRPC-Web
over HTTP/1.1 is supported. Do not freeze compression availability at listener
construction: early_request_filter applies the captured request generation
before module Accept-Encoding processing, and later response processing narrows
eligibility. Ordinary/native-gRPC requests leave the bridge disabled; raw
protocol detection activates it only for canonical gRPC-Web, with WebSocket
precedence. These contracts require equivalent typed module availability under
any proposed storage change.

## Acceptance

Measure module construction separately from Edgion context construction and
from first-use shared-index initialization. Compare allocation calls/bytes,
inline Session size and CPU for ordinary HTTP, compression enabled/disabled,
gRPC-Web, native gRPC and WebSocket traffic. Include concurrent requests and
keepalive reuse so mutable state cannot leak across requests.

Preserve typed get/get_mut, exactly-one module type, filter order, request and
response header rewrites, encoder/trailer completion, cancellation and dropping
request-owned state. Exercise captured configuration update/deletion, H1-only
gRPC-Web activation and raw protocol classification before module rewriting.
Retain current compression eligibility and framing constraints. Module
registration tests alone do not prove wire behavior or allocation improvement.

Relevant original knowledge:
`skills/04-review/cpu-memory/intentional-design/http-request-context-layout-budget.md`,
`skills/04-review/h2-grpc/regression-guards/grpc-web-independent-of-frontend-http2.md`,
`skills/04-review/http1/compatibility-constraints/compression-requires-known-response-length.md`,
and `skills/04-review/listeners/regression-guards/egc-runtime-snapshot-publication.md`.

## Investigation verification

Fresh tests passed: the ordinary context allocation guard, two new_ctx snapshot
tests and two downstream module registration tests. Formatting and workspace
check/Clippy passed; existing warnings are outside this documentation-only diff.
No production code, dependency source or lockfile changed. No Session allocation
benchmark or live protocol test was run.
