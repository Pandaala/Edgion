# HTTP Request Context Memory Optimization

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | in-progress |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

This task reduces the ordinary-request memory and allocation cost of
`EdgionHttpContext` without adding pointer chasing or heap allocation to common
Gateway paths. It separates durable access-log facts from transient execution
state, shares immutable generation data, groups correlated optional state behind
one allocation per enabled scenario, and gives WAF a bounded top-level access-log
block instead of fragmented stage logs.

## Requirements

- Execute each stage as an independently tested and reviewed local commit.
- Do not push commits.
- Preserve request, retry, body, response-streaming, and hot-reload semantics.
- Keep ordinary no-plugin and no-WAF requests allocation-free where practical.
- Keep per-chunk request/response body paths free of logging and unbounded allocation.
- Preserve the existing access-log schema unless a stage explicitly documents a replacement.
- Add WAF as a bounded top-level access-log block and remove WAF stage-log duplication.

## Scope

### In scope

- `EdgionHttpContext` layout and request-local allocations.
- Immutable Gateway/backend selection snapshots.
- RequestInfo common, TLS, and gRPC state separation.
- Plugin, request-body-plugin, response-processing, and route-parameter state aggregation.
- Dedicated bounded WAF access-log state.
- Internal layout/allocation regression measurements and targeted benchmarks.

### Out of scope

- Changes to public resource schemas or plugin configuration.
- New Prometheus metrics.
- Full response-body aggregation.
- External benchmark dependencies or external WAF runtime dependencies.

## Document index

| Document | Status | Notes |
|------|------|------|
| `00-context.md` | done | Required architecture, coding, review, and testing references |
| `01-design.md` | done | State boundaries and optimization order |
| `02-implementation.md` | in-progress | Baselines, verification commands, and acceptance gates |
| `03-subtasks.md` | in-progress | Per-stage progress and commit tracking |

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-gateway/src/ctx.rs` | Request state ownership and layout |
| `edgion-gateway/src/runtime/` | Shared Gateway/listener snapshots |
| `edgion-gateway/src/routes/http/` | Compact HTTP backend selection and request lifecycle wiring |
| `edgion-gateway/src/routes/grpc/` | Compact gRPC backend selection and protocol state |
| `edgion-gateway/src/plugins/runtime/` | Lazy plugin state and stage-log ownership |
| `edgion-gateway/src/plugins/http/waf/` | Structured WAF phase aggregation |
| `edgion-gateway/src/observe/access_log/` | Bounded top-level WAF serialization |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-30 | Keep common hot-path state inline | Boxing universally used state trades cache footprint for a per-request allocation and pointer chase |
| 2026-08-30 | Share immutable compiled state instead of boxing request-owned clones | Generation snapshots already provide stable ownership across hot reload |
| 2026-08-30 | Group correlated optional fields into scenario boxes | One scenario allocation is cheaper and clearer than one box per field |
| 2026-08-30 | Keep WAF outside generic plugin state | WAF spans five engine phases and owns one transaction-wide access-log result |
| 2026-08-30 | Preserve bounded final facts, not transient buffers, for access logging | Logging must not extend the lifetime of body or matcher working state |
