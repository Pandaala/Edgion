# D0055 — Use Clone-cheap Immutable Local Response Bodies

## Meta
- Status: done (2026-09-05)
- Severity: medium
- Discovered in: B0089, Mock audit
- Category: performance-architecture
- Canonical task: completed and removed from `tasks/todo/` on 2026-09-05
- Decision record: [`skills/04-review/cpu-memory/regression-guards/local-response-body-is-clone-cheap-bytes.md`](../../../../skills/04-review/cpu-memory/regression-guards/local-response-body-is-clone-cheap-bytes.md)

## Evidence
`PluginRunningResult::ErrResponse` carries an owned `Option<String>` across the resources-to-Gateway plugin contract. A generation-owned configured body therefore requires a deep clone for every local response. Mock exposes the cost directly, but the enum has many producers and request- and response-stage consumers, so a plugin-local `Arc<str>` or `Bytes` field cannot remove the final copy.

## Why deferred
The representation is a cross-cutting API and wire-semantics decision. It must preserve exact response bytes, local-response framing, gRPC mappings, dynamic body construction, replacement behavior, and the architectural ban on response aggregation or a response-side `BodyStorage`. Changing only Mock would add a second body representation without eliminating the owned enum boundary.

## Integration-test impact
No new integration issue is required. The future task can use enum contract tests, producer/consumer compilation, response-writer tests, and existing frozen end-to-end local-response coverage unless its approved design changes an observable wire contract.

## Resolution (2026-09-05)

`ErrResponse.body` is now `Option<bytes::Bytes>`. Fixed literals use
`Bytes::from_static` (no allocation), generation-owned configured bodies are
prepared as `Bytes` at plugin construction and clone as a refcount bump, and
dynamic bodies keep their single allocation because `Bytes::from(String)` and
`Bytes::from(Vec<u8>)` are moves. The `Serialize`/`Deserialize`/`JsonSchema`
derives on `PluginRunningResult` were the only blocker and had no consumer, so
they were dropped. See the decision record above for the rejected alternatives
and the accepted `DslErrorPolicy::DenyWith` residual.
