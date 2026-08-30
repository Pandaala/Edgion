---
name: pingora-protocol-pumps-remain-separate
description: Accepted architecture boundary for Pingora H1, H2, and custom-protocol pumps. Use when a review proposes deduplicating their response-body batch loops or terminate handling into one shared cross-protocol state machine.
finding_id: pingora-review-31
decision: accepted-tradeoff
---

# Keep Pingora protocol pumps separate

## Decision

Do not unify the H1, H2, and custom-protocol response pumps into one shared
batch-loop abstraction solely to remove duplicated control flow. The visible
similarity is real, but the pumps own different protocol and lifecycle
contracts. A shared state machine would move those differences into callbacks
and flags without removing their complexity, while increasing the regression
surface of Edgion's streaming filters.

Small helpers are allowed only when their inputs, effects, ordering, failure
policy, and cleanup obligations are identical across every caller. A helper
must not hide a protocol-specific branch or make an unsupported custom-session
operation appear supported.

## Protocol boundaries

- H1 owns connection reuse, Content-Length/chunked framing, body-writer flush,
  upgrade transitions, and connection-level termination.
- H2 owns stream-level END_STREAM/RST behavior, multiplexed connection reuse,
  and H2-specific error handling.
- The custom pump delegates framing and connection lifecycle to a user-supplied
  session and carries bidirectional custom messages alongside HTTP-shaped
  request/response data.
- The custom-upstream pump currently fails closed for request-body terminate,
  response-body terminate, non-ordinary request-body disposition, and early
  request-body replay. These are explicit capability boundaries, not missing
  branches to conceal in a common helper. A custom downstream paired with an
  H1 or H2 upstream still runs the corresponding H1/H2 upstream pump and must
  not be conflated with this limitation.

Edgion currently constructs its HTTP service with `ProxyServiceBuilder::new`
and does not install a Pingora custom connector or custom session handler.
ExtProc and Wasm are consumers of the ordinary HTTP filter pipeline, not custom
transports: ExtProc uses a separate tonic connection to its processor, while
Wasm executes in-process. If Edgion adopts a custom protocol later, both plugin
families require an explicit compatibility review rather than inheriting
support from pump deduplication.

## Maintenance policy

1. Keep protocol differences explicit next to the affected control flow.
2. When changing a shared response-body contract, search and review H1, H2,
   custom, cache readback, and subrequest paths together.
3. Prefer cross-protocol contract tests over a common loop abstraction. Cover
   batch termination, natural EOS, errors after termination, cache interaction,
   framing completion, cleanup, and connection/stream reuse where supported.
4. Do not require custom sessions to mimic H1/H2 behavior when the connector
   owns framing or cannot supply the necessary transport fact. Fail closed with
   an explicit unsupported-capability error.
5. Treat a new custom protocol as a designed product capability, with a matrix
   for ExtProc streaming, Wasm body access, request-body terminate,
   response-body injection/terminate, cache, retry, EOS, reuse, and
   custom-message cleanup.

Envoy's Generic Proxy is useful precedent for the boundary: it shares routing,
L7 filtering, connection management, and observability across request/response
protocols while leaving protocol codecs explicit. It does not add an arbitrary
codec to the HTTP Connection Manager or force HTTP and generic protocols into
one pump.

## Re-evaluation triggers

Revisit this decision only when at least one of the following is true:

- Edgion has an approved Dubbo, proprietary RPC, or other generic-proxy design.
- The custom-upstream pump must support response-body terminate, streaming
  request-body processing, or replay.
- A behaviorally significant H1/H2/custom drift is found despite the
  cross-protocol review and test policy.
- The pumps are already being redesigned around a proven protocol-neutral
  state machine for reasons beyond local deduplication.

Until then, duplicated protocol orchestration is an accepted maintenance cost;
cross-protocol unification by itself is not a valid review requirement.
