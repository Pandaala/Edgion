---
name: edgion-skills
description: Root navigation for the Edgion knowledge base. Read this first, then drill into the relevant subtree.
---

# Edgion Skills

> Kubernetes gateway built on Rust + Pingora + Gateway API. Controller–Gateway separation with gRPC config sync.
> Supports HTTP/1.1, HTTP/2, gRPC, TCP, UDP, TLS, WebSocket, and includes a plugin system, load balancing, and TLS/mTLS.

## ⚠️ Read before writing code

**[00-design-rules.md](00-design-rules.md)** — the mandatory graded design-rules index (🔴 MUST /
🟡 SHOULD / 🟢 PREFER) covering redaction, logging-by-binary, observability restraint, cross-cutting
unification, reuse-first, failure policy, schema/transport, workqueue, time, Rust safety, English-only.
Short by design; points to the detailed canon in `03-coding/` and `01-architecture/00-common/`. Scan
its 🔴 rows against your diff before every commit.

## Navigation rules

1. **Progressive disclosure**: this file → category SKILL.md → specific files. Only load the smallest subtree the current task needs.
2. **Three-layer locator**:
   - **Understand the architecture** (how it's implemented internally) → `01-architecture/`
   - **Look up features / config** (how to use / how to configure) → `02-features/`
   - **Write code** (coding conventions) → `03-coding/`
3. **Cross-domain tasks**: modifying a resource Handler typically requires architecture (understand the processing flow) → features (feature/Schema reference) → coding (coding constraints) → testing (verification), loaded layer by layer as needed.
4. **Resource-related tasks** — the two directories have complementary responsibilities and share stable numbers where both pages exist:
   - [01-architecture/05-resources/](01-architecture/05-resources/SKILL.md) — **internal implementation**: Handler flow, requeue links, matching engine, source locations
   - [02-features/03-resources/](02-features/03-resources/SKILL.md) — **external contract**: YAML Schema, field types and default values, configuration examples
   - When changing code you typically need both: check 02 to confirm field definitions in the Schema, check 01 to confirm processing logic in the implementation

## File extensions

- `*.skill` — invokable workflow with a step-numbered structure. Must carry YAML frontmatter (`name:`, `description:`). Loaded as a slash command or by name (see `~/.claude/commands/fix-issue.md` for a working example).
- `SKILL.md` — directory entry point. Routes to sub-files; not invoked directly.
- `*.md` — reference or decision-rule document. Loaded on demand by an upstream skill via "first read X" hints.
- Workflow appendix files use the `<workflow>_<role>.md` shape (e.g., `new_feature_work_flow_templates.md`); they are referenced from the parent `.skill` and not loaded independently.

## Quick locator

| What you want to learn | Direct entry |
|-----------|---------|
| **Design rules (read before coding)** — graded MUST/SHOULD/PREFER index of all cross-cutting rules | [00-design-rules.md](00-design-rules.md) |
| **System architecture** — multi-crate workspace, the three in-repo Controller/Gateway/CLI binaries, internal modules, development guides | [01-architecture/SKILL.md](01-architecture/SKILL.md) |
| **Binary startup and deployment** — CLI arguments, deployment modes | [02-features/01-binary-and-deployment/](02-features/01-binary-and-deployment/SKILL.md) |
| **Config Schema** — Controller (YAML) / Gateway (YAML) configuration | [02-features/02-config/](02-features/02-config/SKILL.md) |
| **Controller authentication & authorization** — bearer-token admin auth, scoped CLI tokens, the (verb,kind) engine, middleware onion, federation policy | [01-architecture/01-controller/11-authentication-authorization.md](01-architecture/01-controller/11-authentication-authorization.md) |
| **Resource feature Schema** — Gateway/Route/TLS/Plugin/Backend/LinkSys | [02-features/03-resources/](02-features/03-resources/SKILL.md) |
| **Resource field safety** — CRD pruning, `schemars` vs serde/conf_sync, resolved Secret redaction, coverage guards | [01-architecture/05-resources/00-schema-transport-contract.md](01-architecture/05-resources/00-schema-transport-contract.md) |
| **conf_sync mTLS cert lifecycle** — auto-CA provisioning, leader-rotation vs per-replica identity-sync split, hot reload, rotation-vs-re-key (trust generation + `TrustScopedStream` revocation), emergency re-key | [01-architecture/03-controller-gateway-link/00-overview.md](01-architecture/03-controller-gateway-link/00-overview.md) (design) + [02-features/02-config/00-controller-config.md](02-features/02-config/00-controller-config.md) (config + runbook) |
| **Webhook provider** — backendRef target, request-templating allow-list (origin/custom + presence), consumer `path_override` API, secret resolution flow | [01-architecture/03-controller-gateway-link/webhook.md](01-architecture/03-controller-gateway-link/webhook.md) |
| **Observability** — Access Log, Metrics, protocol logs | [02-features/04-observability/](02-features/04-observability/SKILL.md) |
| **Annotation reference** — all edgion.io/* keys | [02-features/05-annotations/](02-features/05-annotations/SKILL.md) |
| **Coding conventions** — log IDs, log safety, observability | [03-coding/SKILL.md](03-coding/SKILL.md) |
| **EdgionDSL** — architecture/security, builtin implementation, and user API | [01-architecture/02-gateway/dsl-engine.md](01-architecture/02-gateway/dsl-engine.md) (engine) + [03-coding/dsl/SKILL.md](03-coding/dsl/SKILL.md) (builtins) + `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md` (script API) |
| **Grouped-list design pattern** — `*_groups` schema, atomic flat-field replacement, first-wins, group-in-ok-log iron rule | [01-architecture/00-common/04-grouped-list-pattern.md](01-architecture/00-common/04-grouped-list-pattern.md) |
| **Integration testing** — architecture, running, adding test cases | [05-testing/01-integration-testing.md](05-testing/01-integration-testing.md) |
| **Global review orchestration** — coverage ledger, capacity-aware review waves, and finding output | [10-fully-review/SKILL.md](10-fully-review/SKILL.md); closed decisions live in [04-review/CLOSED-FINDINGS.md](04-review/CLOSED-FINDINGS.md) |
| **User documentation conventions** — writing conventions for authoring/reviewing docs/ documentation | [11-doc/SKILL.md](11-doc/SKILL.md) |
| **Pre-commit / local checks** (fmt + workspace check + workspace clippy + agent-doc validation + SSA-force guard) | [09-misc/SKILL.md](09-misc/SKILL.md) |
| **GitHub Actions / Docker / Release** — tag pushes, Docker images, release-notes files | [09-misc/cicd/02-github-workflow.md](09-misc/cicd/02-github-workflow.md) |
| **Dev-deployment hot-swap** — native image + shell loop + cross-compilation + kubectl cp | [09-misc/03-dev-deploy-hot-replace.md](09-misc/03-dev-deploy-hot-replace.md) |

> For finer-grained locators inside the architecture (Controller/Gateway/Link/resources/plugin development/route matching, etc.) see [01-architecture/SKILL.md](01-architecture/SKILL.md).

## Task locator (what do you want to do?)

Verbed entries for the most common developer workflows. The Quick locator above answers "where is X documented?"; this table answers "I want to do Y, where do I start?".

| Task | Entry point |
|------|-------------|
| Add a new resource type | [01-architecture/01-controller/09-add-new-resource/00-guide.md](01-architecture/01-controller/09-add-new-resource/00-guide.md) |
| Add or rename a resource field | [01-architecture/05-resources/00-schema-transport-contract.md](01-architecture/05-resources/00-schema-transport-contract.md) |
| Develop a new HTTP plugin | [01-architecture/02-gateway/12-edgion-plugin-dev.md](01-architecture/02-gateway/12-edgion-plugin-dev.md) |
| Develop a new Stream plugin | [01-architecture/02-gateway/13-stream-plugin-dev.md](01-architecture/02-gateway/13-stream-plugin-dev.md) |
| Add or debug an EdgionDSL builtin | [03-coding/dsl/SKILL.md](03-coding/dsl/SKILL.md) |
| Debug a runtime issue (404 / 421 / 502 / 503 / Unknown kind) | [06-tracing/00-debugging.md](06-tracing/00-debugging.md) |
| Debug a Controller → Gateway sync issue | [01-architecture/03-controller-gateway-link/SKILL.md](01-architecture/03-controller-gateway-link/SKILL.md) |
| Add a new edgion.io/* annotation | [02-features/05-annotations/00-annotations-overview.md](02-features/05-annotations/00-annotations-overview.md) |
| Run pre-commit / local checks (fmt + workspace check + workspace clippy + agent-doc validation + SSA-force guard) | [09-misc/SKILL.md](09-misc/SKILL.md) |
| Review a PR (per-feature) | [04-review/SKILL.md](04-review/SKILL.md) |
| Run a global codebase review | [10-fully-review/SKILL.md](10-fully-review/SKILL.md) |

## Symptom locator

| Symptom | Where |
|---------|-------|
| Gateway logs `Unknown kind` after Controller restart | [01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md](01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md) |
| Routing returns 404 / 421 / 502 / 503 | [06-tracing/00-debugging.md](06-tracing/00-debugging.md) |

## Key constraints (must read; violations introduce runtime bugs)

> Only "non-obvious, not visible from the code itself, and previously got wrong" constraints are listed here.

| Constraint | Location | Details |
|------|------|------|
| **PluginConditions regex must be compiled in preparse** | `conditions/types.rs`, `edgion_plugins/mod.rs` | `compiled_regex` is `#[serde(skip)]` and is None after deserialization. `compile_conditions()` must be invoked during the `EdgionPlugins::preparse()` phase to finish compilation, otherwise `Regex::new()` is triggered on every request. When adding a new condition type with a regex, also extend the match arm in `Condition::compile()`. See [05-plugin-system.md §Condition execution](01-architecture/02-gateway/05-plugin-system.md) for details. |
| **gRPC H2 grpc-status must be placed in trailers** | Pingora H2 response path | Placing it in response headers causes tonic to return Internal(13); it must go in trailers. |
| **Tracing is forbidden on the data-plane request/connection hot path** | `skills/03-coding/01-log-safety.md` iron rule 3 | The request/connection processing path may only use access log / `PluginLog` / `ctx.err_log` / Metrics; any `tracing::` macro (including `debug!`) is forbidden. Exceptions: configuration loading paths, process startup/shutdown, and access-log send failures in `pg_logging.rs`. Applies to plugin `run_request` / `run_upstream_response` / TLS proxy / stream proxy across the board. |
| **No PluginLog in the `UpstreamResponseBodyFilter` body phase** | `traits/upstream_response_body_filter.rs`, `conditions/evaluator.rs` | The trait signature deliberately omits `&mut PluginLog`; the body phase is invoked once per chunk, and any heap allocation per chunk (PluginLog / String / Vec) accumulates as O(chunks×plugins). When adding a new body-phase filter / wrapper, do not write logs; condition evaluation should use the `evaluate_sync_silent` chain rather than `evaluate_detail_sync`, and ExtensionRef should go through `resolve_runtime_silent`. A long-stream memory leak (issue g2-001) was once introduced because ExtensionRef accidentally called `start_edgion_plugins_log`. See [05-plugin-system.md §No PluginLog in body phase](01-architecture/02-gateway/05-plugin-system.md) for details. |
| **`ConfHandler.partial_update` MUST produce a real incremental diff, never merge-into-full-then-full-set** | [`01-architecture/02-gateway/11-conf-handler-guidelines.md:30`](01-architecture/02-gateway/11-conf-handler-guidelines.md) | The trait signature compiles a "merge + full rebuild" body, but doing so loses updates. If incremental is not yet implemented, mark with `// TODO: incremental rebuild` rather than emulating via full_set. |
| **ConfHandler logs MUST NOT contain Secret / TLS resource bodies** | [`01-architecture/02-gateway/11-conf-handler-guidelines.md:52-54`](01-architecture/02-gateway/11-conf-handler-guidelines.md) | Logs may record metadata (key name, counts, affected scope) and error messages only. The full spec must never appear in any log line. |

## Common practice notes

> Project-wide norms — not hard rules like the table above. Prefer these defaults; deviate with reason and document it.

- **Graceful degrade over panic in long-running paths** — in data-plane filters and control-plane reconcile loops, an unreachable "should-not-happen" branch should normally return a safe default and leave an observability trail (`plugin_log.push_err(...)` on the data plane, `tracing::warn!`/`error!` on the control plane), not `panic!` / `debug_assert!(false, ...)` / `unreachable!()`. Exceptions: startup/init, exhaustive-match dead arms, data-corruption fail-fast. See [03-coding/02-rust-coding-rules.md §Rule 5](03-coding/02-rust-coding-rules.md). Cross-codebase audit: `tasks/todo/graceful-degrade-audit.md`.

## Directory overview

| # | Directory | Purpose |
|---|------|------|
| 01 | [architecture/](01-architecture/SKILL.md) | System architecture + development guides: Controller, Gateway, gRPC sync, resource processing, plugin/resource/connector development |
| 02 | [features/](02-features/SKILL.md) | Feature and configuration reference: binary deployment, configuration Schema, resource feature Schema, observability, annotations |
| 03 | [coding/](03-coding/SKILL.md) | Coding conventions: log IDs, log safety, observability (Access Log / Metrics / Tracing) |
| 04 | [review/](04-review/SKILL.md) | Review notes: false-positive determinations, observability audits |
| 05 | [testing/](05-testing/SKILL.md) | Testing: unit tests, integration tests, K8s tests, special-purpose tests |
| 06 | [tracing/](06-tracing/SKILL.md) | Debugging — symptom-to-tool guide: Admin API, edgion-cli, common runtime issues (not distributed tracing) |
| 07 | [tasks/](07-tasks/SKILL.md) | Task management: directory rules, templates, lifecycle phases, completion checklists |
| 08 | [kubernetes/](08-kubernetes/SKILL.md) | Kubernetes-related: Gateway API compatibility, API version upgrade strategy |
| 09 | [misc/](09-misc/SKILL.md) | Miscellaneous (TLS troubleshooting, etc.) |
| 10 | [fully-review/](10-fully-review/SKILL.md) | Global review orchestration: scope ledger, capacity-aware waves, finding format, and coverage closure |
| 11 | [11-doc/](11-doc/SKILL.md) | User documentation conventions: directory layout, writing style, page templates, multilingual, version management, review checklist |

## Development lifecycle

See [`07-tasks/SKILL.md`](07-tasks/SKILL.md) for the full lifecycle phase table (Analysis → Design → Implementation → Test → Review).

## User documentation

Located under `docs/`, organized by language (en, zh-CN). Directory-level `README.md`
files and `docs/.vitepress/config.ts` are the maintained navigation sources.

When authoring or reviewing user documentation, follow the writing conventions in [11-doc/SKILL.md](11-doc/SKILL.md).
