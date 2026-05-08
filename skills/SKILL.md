---
name: edgion-skills
description: Root navigation for the Edgion knowledge base. Read this first, then drill into the relevant subtree.
---

# Edgion Skills

> Kubernetes gateway built on Rust + Pingora + Gateway API. Controller–Gateway separation with gRPC config sync.
> Supports HTTP/1.1, HTTP/2, gRPC, TCP, UDP, TLS, WebSocket, and includes a plugin system, load balancing, and TLS/mTLS.

## Navigation rules

1. **Progressive disclosure**: this file → category SKILL.md → specific files. Only load the smallest subtree the current task needs.
2. **Three-layer locator**:
   - **Understand the architecture** (how it's implemented internally) → `01-architecture/`
   - **Look up features / config** (how to use / how to configure) → `02-features/`
   - **Write code** (coding conventions) → `03-coding/`
3. **Cross-domain tasks**: modifying a resource Handler typically requires architecture (understand the processing flow) → features (feature/Schema reference) → coding (coding constraints) → testing (verification), loaded layer by layer as needed.
4. **Resource-related tasks** — the two directories are aligned by number with complementary responsibilities:
   - [01-architecture/05-resources/](01-architecture/05-resources/SKILL.md) — **internal implementation**: Handler flow, requeue links, matching engine, source locations
   - [02-features/03-resources/](02-features/03-resources/SKILL.md) — **external contract**: YAML Schema, field types and default values, configuration examples
   - When changing code you typically need both: check 02 to confirm field definitions in the Schema, check 01 to confirm processing logic in the implementation

## Quick locator

| What you want to learn | Direct entry |
|-----------|---------|
| **System architecture** — relationship between the three Controller/Gateway/Ctl bins, internal modules, development guides | [01-architecture/SKILL.md](01-architecture/SKILL.md) |
| **Binary startup and deployment** — CLI arguments, deployment modes | [02-features/01-binary-and-deployment/](02-features/01-binary-and-deployment/SKILL.md) |
| **TOML config Schema** — Controller/Gateway configuration | [02-features/02-config/](02-features/02-config/SKILL.md) |
| **Resource feature Schema** — Gateway/Route/TLS/Plugin/Backend/LinkSys | [02-features/03-resources/](02-features/03-resources/SKILL.md) |
| **Observability** — Access Log, Metrics, protocol logs | [02-features/04-observability/](02-features/04-observability/SKILL.md) |
| **Annotation reference** — all edgion.io/* keys | [02-features/05-annotations/](02-features/05-annotations/SKILL.md) |
| **Coding conventions** — log IDs, log safety, observability | [03-coding/SKILL.md](03-coding/SKILL.md) |
| **Integration testing** — architecture, running, adding test cases | [05-testing/01-integration-testing.md](05-testing/01-integration-testing.md) |
| **Global review** — security/correctness/observability checklists + closed false-positive entries | [10-fully-review/SKILL.md](10-fully-review/SKILL.md) |
| **User documentation conventions** — writing conventions for authoring/reviewing docs/ documentation | [11-doc/SKILL.md](11-doc/SKILL.md) |
| **CI / release automation** — GitHub Release, tag pushes, Docker images, location of release-notes files | [09-misc/cicd/02-github-workflow.md](09-misc/cicd/02-github-workflow.md) |
| **Center federation persistence** — SQLite `controllers` table, CenterDb, database configuration (PM data has switched to reverse Watch and no longer uses SQLite cache) | [09-misc/center-db.md](09-misc/center-db.md) |
| **Center reverse Watch architecture** — CenterWatchCache, CenterMetaDataStore, federation gRPC bidirectional stream PM data flow | [09-misc/center-db.md](09-misc/center-db.md) |
| **Dev-deployment hot-swap** — native image + shell loop + cross-compilation + kubectl cp | [09-misc/03-dev-deploy-hot-replace.md](09-misc/03-dev-deploy-hot-replace.md) |

> For finer-grained locators inside the architecture (Controller/Gateway/Link/resources/plugin development/route matching, etc.) see [01-architecture/SKILL.md](01-architecture/SKILL.md).

## Key constraints (must read; violations introduce runtime bugs)

> Only "non-obvious, not visible from the code itself, and previously got wrong" constraints are listed here.

| Constraint | Location | Details |
|------|------|------|
| **PluginConditions regex must be compiled in preparse** | `conditions/types.rs`, `edgion_plugins/mod.rs` | `compiled_regex` is `#[serde(skip)]` and is None after deserialization. `compile_conditions()` must be invoked during the `EdgionPlugins::preparse()` phase to finish compilation, otherwise `Regex::new()` is triggered on every request. When adding a new condition type with a regex, also extend the match arm in `Condition::compile()`. See [05-plugin-system.md §Condition execution](01-architecture/02-gateway/05-plugin-system.md) for details. |
| **gRPC H2 grpc-status must be placed in trailers** | Pingora H2 response path | Placing it in response headers causes tonic to return Internal(13); it must go in trailers. |
| **Tracing is forbidden on the data-plane request/connection hot path** | `skills/03-coding/01-log-safety.md` iron rule 3 | The request/connection processing path may only use access log / `PluginLog` / `ctx.err_log` / Metrics; any `tracing::` macro (including `debug!`) is forbidden. Exceptions: configuration loading paths, process startup/shutdown, and access-log send failures in `pg_logging.rs`. Applies to plugin `run_request` / `run_upstream_response` / TLS proxy / stream proxy across the board. |
| **No PluginLog in the `UpstreamResponseBodyFilter` body phase** | `traits/upstream_response_body_filter.rs`, `conditions/evaluator.rs` | The trait signature deliberately omits `&mut PluginLog`; the body phase is invoked once per chunk, and any heap allocation per chunk (PluginLog / String / Vec) accumulates as O(chunks×plugins). When adding a new body-phase filter / wrapper, do not write logs; condition evaluation should use the `evaluate_sync_silent` chain rather than `evaluate_detail_sync`, and ExtensionRef should go through `resolve_runtime_silent`. A long-stream memory leak (issue g2-001) was once introduced because ExtensionRef accidentally called `start_edgion_plugins_log`. See [05-plugin-system.md §No PluginLog in body phase](01-architecture/02-gateway/05-plugin-system.md) for details. |

## Directory overview

| # | Directory | Purpose |
|---|------|------|
| 01 | [architecture/](01-architecture/SKILL.md) | System architecture + development guides: Controller, Gateway, gRPC sync, resource processing, plugin/resource/connector development |
| 02 | [features/](02-features/SKILL.md) | Feature and configuration reference: binary deployment, configuration Schema, resource feature Schema, observability, annotations |
| 03 | [coding/](03-coding/SKILL.md) | Coding conventions: log IDs, log safety, observability (Access Log / Metrics / Tracing) |
| 04 | [review/](04-review/SKILL.md) | Review notes: false-positive determinations, observability audits |
| 05 | [testing/](05-testing/SKILL.md) | Testing: unit tests, integration tests, K8s tests, special-purpose tests |
| 06 | [tracing/](06-tracing/SKILL.md) | Debugging: Admin API, edgion-cli, common issues |
| 07 | [tasks/](07-tasks/SKILL.md) | Task management: directory rules, templates, lifecycle phases, completion checklists |
| 08 | [kubernetes/](08-kubernetes/SKILL.md) | Kubernetes-related: Gateway API compatibility, API version upgrade strategy |
| 09 | [misc/](09-misc/) | Miscellaneous (TLS troubleshooting, etc.) |
| 10 | [fully-review/](10-fully-review/SKILL.md) | Global review guide: per-dimension checklists, security misjudgement rules, closed entries |
| 11 | [11-doc/](11-doc/SKILL.md) | User documentation conventions: directory layout, writing style, page templates, multilingual, version management, review checklist |

## Development lifecycle quick reference

Tasks progress phase by phase, with the corresponding skills loaded at each phase (see [07-tasks/SKILL.md](07-tasks/SKILL.md) for details):

| Phase | What to do | Load |
|-------|--------|------|
| 1 Analysis | Requirements analysis, impact assessment | `01-architecture/` |
| 2 Design | Solution design, Schema confirmation | `01-architecture/`, `02-features/` |
| 3 Implementation | Coding | `01-architecture/` (development guide), `02-features/` (config Schema), `03-coding/` |
| 4 Testing | Test verification | `05-testing/` |
| 5 Review | Code review | `04-review/`, `10-fully-review/` (global review or when reviewing AI reports) |

## User documentation

Located under `docs/`, organized by language (en, zh-CN, ja). The complete directory tree is in [docs/DIRECTORY.md](../docs/DIRECTORY.md).

When authoring or reviewing user documentation, follow the writing conventions in [11-doc/SKILL.md](11-doc/SKILL.md).
