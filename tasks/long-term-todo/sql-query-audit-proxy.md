# SQL query audit and database security proxy

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

Investigate and, only if later authorized, design a MySQL/PostgreSQL protocol-aware proxy in Edgion that can extract SQL text at the network edge for structured audit and optional injection denial, without changing application clients. This is a third product surface, not an extension of today's HTTP/gRPC API Gateway or AI Gateway. The current `StreamPlugin` contract is connection admission only (`Allow`/`Deny` from peer IP and listener port) and cannot inspect or mutate TCP bytes. The hard parts are a panic-safe stream codec, session-stateful protocol parsing (handshake, auth, prepared statements, compression), and competing with dedicated poolers and audit appliances rather than copying them.

## Why this stays long-term

Connection pooling, failover, and read/write split are already served by ProxySQL, PgBouncer, AWS RDS Proxy, and cloud auth connectors. Those are not Edgion's opening. The remaining buyer-shaped use is finance / compliance query audit and a thin SQL-injection deny at a controlled network boundary. That market is real, but buyers today purchase dedicated DB-audit products, not a general API gateway. Resume only with a named customer or an explicit product-scope change.

## Requirements

- Parse MySQL and PostgreSQL frontend query packets far enough to recover the SQL text of ordinary and prepared-statement executions.
- Emit a structured audit record: authenticated identity, time, target instance, statement class, and SQL text, without leaking secrets or full result sets.
- Optionally deny a bounded class of injection patterns before the packet reaches the database. Fail closed on parse failure for the deny path; never crash the proxy worker.
- Keep the default path as inspect-and-forward. Do not become a connection multiplexer, query cache, or read/write splitter in the first shipment.
- Reuse a future stream byte-filter seam if one exists; do not invent a second TCP interception stack.

## Scope

### In scope

- MySQL and PostgreSQL wire-protocol query extraction on the Gateway stream path.
- Query-level audit logs and a narrow, config-driven deny list.
- Panic isolation around the codec and parser (`catch_unwind` / bounded state machines).
- A later design for identity (mTLS / SPIFFE / proxy-protocol) attached to each audit record.

### Out of scope

- Redis RESP proxying, key routing, or command metrics. That slice was dropped from the former combined backlog.
- Kafka protocol proxying. Tracked separately in `tasks/long-term-todo/kafka-protocol-edge-acl.md`.
- Competing with RDS Proxy / ProxySQL / PgBouncer on pooling, pinning, or failover.
- Replacing dedicated database-audit appliances or building a full SQL AST rewriter.
- MCP / A2A or any other protocol-gateway product.

## Shared prerequisite

The first authorized implementation of either this task or the Kafka task must design a Stream Filter SDK: `StreamPlugin` today cannot see socket buffers. That seam is a dependency, not a third product, and must stay protocol-agnostic.

## Related guidance

- `AGENTS.md` product scope (API Gateway + AI Gateway only today)
- `skills/01-architecture/02-gateway/13-stream-plugin-dev.md`
- `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs`
- `tasks/long-term-todo/kafka-protocol-edge-acl.md`
- Split from `tasks/todo/database-middleware-proxies.md` on 2026-09-05

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-05 | Split SQL out of the combined Redis/MySQL/Kafka todo and park it as long-term. | Query audit has a real compliance buyer; Redis command routing does not justify a gateway product. Implementation waits for an explicit product-scope decision. |
