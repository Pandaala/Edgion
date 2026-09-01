# D0055 — Use Clone-cheap Immutable Local Response Bodies

## Meta
- Status: needs-discussion
- Severity: medium
- Discovered in: B0089, Mock audit
- Category: performance-architecture
- Canonical task: [`tasks/todo/use-clone-cheap-local-response-bodies.md`](../../../todo/use-clone-cheap-local-response-bodies.md)

## Evidence
`PluginRunningResult::ErrResponse` carries an owned `Option<String>` across the resources-to-Gateway plugin contract. A generation-owned configured body therefore requires a deep clone for every local response. Mock exposes the cost directly, but the enum has many producers and request- and response-stage consumers, so a plugin-local `Arc<str>` or `Bytes` field cannot remove the final copy.

## Why deferred
The representation is a cross-cutting API and wire-semantics decision. It must preserve exact response bytes, local-response framing, gRPC mappings, dynamic body construction, replacement behavior, and the architectural ban on response aggregation or a response-side `BodyStorage`. Changing only Mock would add a second body representation without eliminating the owned enum boundary.

## Integration-test impact
No new integration issue is required. The future task can use enum contract tests, producer/consumer compilation, response-writer tests, and existing frozen end-to-end local-response coverage unless its approved design changes an observable wire contract.
