---
name: coding-standards
description: Coding standards skill for Edgion. Use when reviewing or writing code that touches logging, tracing IDs, log safety, access logs, metrics, structured tracing, or shared project-wide implementation rules.
---

# Coding Standards

> Edgion project-wide coding standards. Covers logging and ID propagation, sensitive-information leakage prevention, control-plane / data-plane log separation, and observability design (Access Log / Metrics / control-plane Tracing).
> These standards are mandatory. New code **must** comply; existing code is brought into alignment incrementally as it is modified.

## Core principles

1. **Single-line access log reconstruction** — one access log entry contains enough information to locate a problem without flipping through other logs
2. **Record only key checkpoints** — record final decisions and exception paths; do not record process steps or intermediate state
3. **Avoid metrics explosion** — no histograms, strict label cardinality, unified management
4. **Zero tracing on the data plane** — `tracing::*` macros are forbidden on the data-plane hot path

## Information layering

| Layer | Tool | Target audience | Retention |
|------|------|---------|---------|
| Request granularity | access log (JSON) | ops troubleshooting, audit | days~weeks |
| System events | `tracing::info/warn/error` | developer debugging, alerts | hours~days |
| Performance aggregation | Prometheus metrics | SRE, Grafana | months |

## File list

### Logging and safety

| File | Topic |
|------|------|
| [00-logging-and-tracing-ids.md](00-logging-and-tracing-ids.md) | Log ID propagation: rv / sv / key_name triple, ensures control plane → data plane correlation |
| [01-log-safety.md](01-log-safety.md) | Log safety: no sensitive info in logs, no config leakage, tracing forbidden on data plane |

### Rust coding rules

| File | Topic |
|------|------|
| [02-rust-coding-rules.md](02-rust-coding-rules.md) | Rust coding rules: short import paths, unwrap ban, judicious Clone, actionable error messages |

### Observability — [observability/](observability/)

| File | Topic |
|------|------|
| [observability/00-access-log.md](observability/00-access-log.md) | Access Log design: field structure, PluginLog format (including `push_err` error-entry API), common-scenario quick reference |
| [observability/01-metrics.md](observability/01-metrics.md) | Metrics rules: how to add, label constraints, Test Metrics, prohibitions |
| [observability/02-tracing-and-logging.md](observability/02-tracing-and-logging.md) | Control-plane logging: structured Tracing, level selection, error context, instrument naming |

## Pre-commit self-check list

### Log safety
- [ ] Are newly added `tracing::info!/warn!/error!` calls on the data-plane hot path? If so, switch to ctx-based
- [ ] Do logs contain sensitive data such as Secret / Token / keys?
- [ ] Do logs contain a full resource spec / backend address? If so, downgrade to debug
- [ ] Do new control-plane logs include `kind` + `name/namespace` + `rv`?
- [ ] If the resource has been synced to the data plane, is `sv` present in the access log?

### Rust coding
- [ ] Any inline `crate::` path more than 2 segments deep? Extract to a `use` import
- [ ] Any bare `unwrap()`? Switch to `?` / `expect("reason")` / `match`
- [ ] Do `expect()` messages explain why panic is impossible?
- [ ] Any unnecessary `.clone()`? Can a reference or `Arc` be used instead?
- [ ] Do error messages carry enough context?

### Observability
- [ ] Each PluginLog entry ≤ 40 bytes, ending with `; `
- [ ] Error paths go through `plugin_log.push_err("<static kind>")`, not `push(&format!("err: {}", e))` (avoids token / claim leakage)
- [ ] No debug information written into PluginLog (use `tracing::debug!` instead)
- [ ] New metrics are managed via the `GatewayMetrics` struct, not the `metrics::counter!()` macro directly
- [ ] No Histogram type for new metrics
- [ ] Label value domain is bounded (≤ tens), no high-cardinality labels
- [ ] Every +1 path on a Gauge has a matching -1
- [ ] New access log fields actually carry troubleshooting value
- [ ] Tracing logs include a `component` field; error/warn carry enough context to locate the issue
- [ ] Observability operations on hot paths are low-overhead
- [ ] Do new keys added to `ctx_map` potentially contain sensitive information? If so, filter them
- [ ] Do plugin stage_logs record full username / token? Record only a prefix

### DSL plugin implementation — [dsl/](dsl/)

> See [dsl/SKILL.md](dsl/SKILL.md) for the sub-directory index and cross-links to the DSL engine architecture, URI normalization, and the Value-type design memo.

| File | Topic |
|------|------|
| [dsl/builtins-path.md](dsl/builtins-path.md) | DSL path/URI variable reference: `req.path`, `req.uri`, `req.query`, and the difference between them |
| [dsl/builtin-error-semantics.md](dsl/builtin-error-semantics.md) | DSL builtin error-handling contract: silent-fallback vs `RuntimeError::ApiError` grouping, lock-in tests, "adding a new builtin" decision flow |
| [dsl/http-call.md](dsl/http-call.md) | `http.call()` builtin: async outbound HTTP, `Value::Map`, SSRF guard, config fields, test patterns |
| [dsl/redis-builtin.md](dsl/redis-builtin.md) | `redis.*` builtins: async LinkSys Redis interaction, named alias lookup, return `Map {ok, value, error}` |

## Relation to other Skills

- **review/** — Review uses these standards as the criteria, including the observability review checklist
- **01-architecture/** — The data-plane / control-plane split in the architecture defines the logging boundary
