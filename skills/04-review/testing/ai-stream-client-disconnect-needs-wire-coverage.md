# AI streaming client disconnect needs wire coverage

## Decision

AI client-disconnect behavior is a cross-process contract. A usage-finalizer
unit test cannot prove that dropping a downstream response body cancels the
gateway's upstream read, reaches the universal logging boundary once, settles
one incomplete attempt, and reconciles request quota.

The SingleProvider integration suite uses the quota-enabled route and an H2
client. It reads three non-terminal SSE frames from a paced long provider stream
and drops the response body before the provider emits usage or `[DONE]`.

The test server records only an opaque trace ID and bounded lifecycle counters.
Its producer marks cancellation when the gateway drops the upstream body
receiver, so continuing to drain the provider after the client disconnects is a
test failure. The integration-only access-log store exposes a per-trace
`store_count`; this distinguishes one logging-boundary write from an accidental
second write even though the store's trace-keyed map would otherwise overwrite
it.

## Required assertions

- The provider producer is cancelled before all 103 chunks are queued.
- Exactly one access-log write contains one untruncated attempt with
  `failure=incomplete_response` and estimated, non-zero input/output tokens.
- The Redis quota bucket equals that attempt's estimated token total, and the
  final quota status agrees with whether the configured limit was exceeded.
- No terminal usage or `[DONE]` frame reaches the client before it disconnects.

Unused provider modes are not coverage. The redundant single-chunk `abort`
mode and speculative `/v1/models` observation/response branch were removed;
upstream abort remains covered by the dedicated resilience fixture.
