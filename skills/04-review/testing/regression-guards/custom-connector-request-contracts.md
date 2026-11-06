---
name: custom-connector-request-contracts
description: Use when checking custom-connector request action, framing and source EOF coverage in the Pingora fork.
knowledge_type: regression-guards
---

# Custom connector request contracts need pump coverage

## Conclusion and scope

`03-fork-layer-coverage-for-unreachable-paths` was a fork test-coverage task,
not an Edgion production defect. Its claim that no custom session driver
existed was disproved by the current fork harness. Shared relay unit tests
alone did not cover the three requested custom-pump scenarios.

The test work now drives actual custom sessions for typed request termination,
pre-header rejection of `Bodyless` and `Streamed`, and source EOF with a stale
done predicate. A successful-upload control verifies the forwarding probes.
The owning detailed evidence is in the Pandaala/pingora repository at
`edgion-changes/review/custom/custom-request-contract-coverage.md`.

## Evidence and limits

On 2026-10-06, six new tests each passed 10 isolated runs. Eight mutations
demonstrated specific condition and observer boundaries; all were restored in
an isolated worktree. The H1 custom upload tests deliberately force partial
chunk callbacks and inspect cumulative forwarded bytes.

Item A's historical H1 reachability restrictions were also disproved: the
current and locked fork parse and serialize authority-form CONNECT requests.
New H1 tests cover no length, Content-Length and chunked request framing under
Bodyless coercion, and pre-header rejection under the current frozen Streamed
contract. Existing H2 CONNECT tunnel coverage remains. The tests do not claim
bidirectional H1 tunneling after 200 or a complete protocol matrix; details and
historical corrections remain in the owning fork record.

Historically, the fork baseline was `bd08429` plus local test changes while
Edgion's `Cargo.lock` still selected `0b0d8bb`, which did not contain the new
tests; this task did not publish the fork, update the lockfile, add CONNECT
routing, or introduce a custom connector into the gateway.

The current lock selects
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`. Source inspection at
that revision on 2026-11-06, after the fork was rebased onto upstream 0.9.1,
found the six named tests committed
(`pingora-proxy/tests/upstream_response_body_sink/custom_request_contract.rs`
and `pingora-proxy/tests/seam/connect.rs`), the evidence record above present,
and the custom pump contracts unchanged in `pingora-proxy/src/proxy_custom.rs`:
non-`Ordinary` dispositions fail with `InternalError` before the upstream
header write, a request-body `Terminate` outcome fails with `InternalError`,
and `request_relay.rs` treats source EOF as authoritative over a stale
`is_body_done()`. These tests were not run for this re-verification. That
fork record's closing sentence still says Edgion locks `0b0d8bb`; it is stale.
Edgion still installs no custom connector.

## Re-evaluation

Recheck the fork tests if Edgion installs a custom connector, the custom pump
gains framing/termination support, or request EOF and idle monitoring change.
Passing fork tests do not establish a deployed capability or remove that
product-design boundary.

## Resolution references

- `03-fork-layer-coverage-for-unreachable-paths` — completed fork coverage;
  historical missing-driver premise corrected, production semantics unchanged.
