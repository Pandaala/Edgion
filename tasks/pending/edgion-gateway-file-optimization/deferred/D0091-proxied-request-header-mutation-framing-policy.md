# D0091 — Unify Proxied Request-Header Mutation and Framing Policy

## Meta

- Status: needs-discussion
- Severity: high
- Discovered in: B0127 Gateway API header-modifier analysis
- Category: request-framing
- Canonical task: [`tasks/todo/unify-proxied-request-header-mutation-and-framing-policy.md`](../../../todo/unify-proxied-request-header-mutation-and-framing-policy.md)

## Evidence

Request plugins mutate the downstream header before Pingora constructs the upstream request. Pingora removes standard and dynamically nominated hop-by-hop fields, but an operator-set `Content-Length` survives; Edgion's final body-framing helper is a no-op when no modified-body length exists, and the HTTP/1 writer can therefore send a body that diverges from the declared length. D0068 is response-only and does not own this boundary.

## Why deferred

A correct policy must cover every request-header producer, body transformer, HTTP version, gRPC, retries, bodyless and streamed bodies, and the final Pingora session/request boundary. B0127 must not invent a plugin-local deny-list or framing mechanism.

## Integration-test impact

First add deterministic boundary and fork-level tests. Create a standalone wire integration issue only if local tests cannot prove the final framing behavior; do not modify existing frozen integration files.
