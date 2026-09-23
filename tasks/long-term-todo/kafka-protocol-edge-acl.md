# Kafka protocol edge ACL proxy

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-05 |
| Status | long-term |
| Type | feature |
| Priority | P3 |
| Issue | N/A |
| Horizon | Product bet. Do not implement until explicitly authorized. |

## AI guide summary

Investigate and, only if later authorized, design a Kafka-protocol-aware edge proxy in Edgion that can read Produce/Fetch (and related) frames, extract topic names, and allow or deny the call from a workload identity before the request reaches a broker. Broker-native ACLs and Confluent Gateway already cover most production needs; Edgion's only distinct bet is a network-edge enforcement point that applications do not have to know about. The current `StreamPlugin` contract cannot see TCP bytes. The hard parts are Kafka API-version negotiation, metadata/bootstrap rewrite so clients never learn real broker addresses, transactional and consumer-group APIs, and staying compatible enough that ordinary clients need no code change.

## Why this stays long-term

Kafka authorization in production is usually done on the brokers (SASL, mTLS, ACLs). Confluent Gateway (2025–2026) is a licensed protocol proxy aimed at migration, virtual bootstrap, and credential swapping. A generic API gateway is not the default place teams buy this. Resume only with a named customer, a mesh/SPIFFE story that brokers cannot express, or an explicit product-scope change.

## Requirements

- Parse Kafka request frames far enough to recover API key, API version, and topic names for Produce, Fetch, and the other topic-bearing APIs that an edge deny must cover.
- Authorize each call against a configured topic ACL bound to a workload identity (mTLS / SPIFFE). Deny at the edge with a Kafka-protocol error, not an HTTP status.
- Present stable virtual bootstrap/broker endpoints so cluster membership changes do not leak through to clients. Metadata responses must be rewritten.
- Fail closed on parse or identity failure for the deny path; never panic the stream worker.
- Reuse a future stream byte-filter seam if one exists; do not invent a second TCP interception stack.

## Scope

### In scope

- Kafka binary protocol inspection on the Gateway stream path.
- Topic-level allow/deny at the edge, plus audit of denied and optionally allowed calls.
- Metadata/bootstrap virtualization required for clients to treat Edgion as the cluster entry.
- Panic isolation around the codec and version-skew handling.

### Out of scope

- SQL / MySQL / PostgreSQL inspection. Tracked separately in `tasks/long-term-todo/sql-query-audit-proxy.md`.
- Redis RESP proxying. That slice was dropped from the former combined backlog.
- Replacing broker ACLs, Schema Registry governance, or a full Kafka control plane.
- HTTP Kafka REST proxies, UI consoles, or record-payload inspection.
- MCP / A2A or any other protocol-gateway product.

## Shared prerequisite

The first authorized implementation of either this task or the SQL task must design a Stream Filter SDK: `StreamPlugin` today cannot see socket buffers. That seam is a dependency, not a third product, and must stay protocol-agnostic.

## Related guidance

- `AGENTS.md` product scope (API Gateway + AI Gateway only today)
- `skills/01-architecture/02-gateway/13-stream-plugin-dev.md`
- `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs`
- `tasks/long-term-todo/sql-query-audit-proxy.md`
- Split from `tasks/todo/database-middleware-proxies.md` on 2026-09-05

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-05 | Split Kafka out of the combined Redis/MySQL/Kafka todo and park it as long-term. | Topic-level edge ACL and virtual bootstrap are a real, separately staffed product; they should not share a file with SQL audit or Redis routing. Implementation waits for an explicit product-scope decision. |
