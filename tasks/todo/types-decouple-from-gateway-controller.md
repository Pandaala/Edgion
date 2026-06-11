# Decouple `src/types/` from `core::gateway` / `core::controller`

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-06-11 |
| Status | todo |
| Type | refactor |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

> Enforce a hard layering invariant: **`src/types/` must not depend on any
> `core::gateway` or `core::controller` code.** Today the resource/config types fuse
> their serializable schema with *compiled runtime artifacts* (`BackendSelector`,
> `PluginRuntime`, `StreamPluginRuntime`, `DslPlugin`, `PluginConditions`) and reach
> sideways into gateway helpers/global config. This makes `types` a non-portable leaf.
> This task peels the pure data model (stays in `types`) from the gateway runtime
> (moves into `core::gateway`), removes a dead `core::controller` re-export, and adds a
> CI guard so the invariant cannot regress. **This is the prerequisite for extracting
> `src/types` into a standalone crate shared by Edgion and the future EdgionCenter repo**
> — see `center-split-hub-management-plane.md`. The hardest single point is the DSL VM
> embedded in `plugin_configs/dsl.rs`.

## Goal / Invariant

After this task:

```
grep -rE "use crate::core::(gateway|controller)" src/types  →  (no matches)
grep -rE "crate::core::(gateway|controller)"     src/types  →  (no matches)
```

`src/types/` becomes a clean leaf that depends only on `core::common` helpers (and
external crates). `core::common` itself may later be split further, but that is out of
scope here.

## Requirements (verbatim intent)

- `src/types/` 下面不能依赖任何 `core::gateway` / `core::controller`.
- Do this first, before the `types` crate extraction and the Center repo split.

## Scope

### In scope
- Remove every `core::gateway` / `core::controller` reference from `src/types/`
  (15 files, ~30 references — full inventory below).
- Relocate gateway runtime state currently living under `types/` into `core::gateway`.
- Relocate pure helpers/config that `types` legitimately needs to a layer `types` may
  depend on (`core::common` or the `types` crate itself).
- Add a CI/lint guard enforcing the invariant.

### Out of scope
- Actually creating the standalone `types` cargo crate (workspace split) — follow-up.
- Extracting Center into the EdgionCenter repo — follow-up.
- Splitting `core::common` further, reworking auth, or the DSL VM internals beyond what
  is needed to break the `types → gateway` edge.
- Any behavior change. This is a pure structural refactor; runtime behavior must be
  identical and all existing tests must pass.

## Inventory — every offending edge

### Group A — `ctx.rs` (runtime context misplaced under `types/`)
`src/types/ctx.rs` (1546 lines) holds gateway **request/runtime context**
(`EdgionHttpContext`, `RequestInfo`, `UpstreamInfo`, `BackendContext`, …) and imports:
`gateway::backends::EdgionService`, `gateway::lb::edgion_backend::EdgionBackend`,
`gateway::lb::LbCommit`, `gateway::observe::logs::LogBuffer`,
`gateway::plugins::runtime::PluginRuntime`, `gateway::plugins::{EdgionPluginsLog, StageLogs}`,
`gateway::routes::grpc::GrpcRouteRuleUnit`, `gateway::routes::HttpRouteRuleUnit`,
`gateway::runtime::GatewayInfo`, `gateway::path_normalization::{normalize_path, PathNormalizationConfig}`.

**Consumers: gateway only** (16 files, all under `core/gateway`; nothing else imports
`types::ctx`). → **Remediation: move `ctx.rs` out of `types/` into `core::gateway`**
(e.g. `core::gateway::ctx`). It is runtime state, not a portable type. Lowest-risk fix.

### Group B — route types (compiled runtime fused into schema)
`http_route.rs`, `http_route_preparse.rs`, `grpc_route.rs`, `tcp_route.rs`,
`tls_route.rs`, `udp_route.rs` each hold `#[serde(skip)]` runtime fields:
`backend_finder: BackendSelector<…>` (`gateway::lb::BackendSelector`),
`plugin_runtime: Arc<PluginRuntime>` / `StreamPluginRuntime` (`gateway::plugins::*`),
plus `gateway::routes::common::TcpKeepaliveConfig` and
`gateway::path_normalization::normalize_config_path`.

→ **Remediation:** separate the serializable route **schema** (stays in `types`) from the
**prepared/compiled route** that carries `BackendSelector` / `PluginRuntime`. Move the
`#[serde(skip)]` runtime fields into a gateway-side wrapper (e.g. a `PreparedHttpRoute`
in `core::gateway::routes`). `TcpKeepaliveConfig` is pure config in
`gateway::routes::common.rs:188` → relocate down (to `types` or `core::common`).
`path_normalization` helpers → relocate down or have gateway pass already-normalized data.

### Group C — plugin configs (runtime handles + DSL VM)
- `edgion_plugins/mod.rs`, `edgion_stream_plugins/mod.rs`: `PluginRuntime` /
  `StreamPluginRuntime` / `TlsRoutePluginRuntime` skip-fields → move runtime out.
- `edgion_plugins/entry.rs`: `gateway::plugins::runtime::conditions::PluginConditions`
  → move compiled conditions out; keep raw condition config in `types`.
- `plugin_configs/dsl.rs` **(hardest)**: re-exports `DslConfig` and uses
  `gateway::plugins::http::dsl::lang::vm::DslErrorPolicy` and
  `gateway::plugins::http::dsl::plugin::DslPlugin`. The compiled DSL plugin/VM is embedded
  in the config type. → Keep `DslConfig` (schema) in `types`; move the compiled
  `DslPlugin` / VM / `DslErrorPolicy` handling into gateway. Verify where `DslConfig` is
  actually defined first (it may already belong in `types`).
- `plugin_configs/rate_limit.rs`: re-exports `gateway::cli::config` items and calls
  `set_gateway_instance_count` (test) — relocate the shared constants/config down or
  invert the dependency.
- `plugin_configs/request_restriction.rs`: `path_normalization::normalize_config_path`
  and `gateway::cli::config::get_path_normalization_config` → same helper relocation as
  Group B.

### Group D — `types/mod.rs` controller re-export (dead)
`src/types/mod.rs:61` `pub use crate::core::controller::conf_mgr::{CenterApi as
ResourceStore, ConfEntry as ResourceEntry, ConfStoreError as ResourceStoreError}`.
These aliases are **used by nobody** → delete outright.

### Group E — inline (non-`use`) path references
`ctx.rs` (lines ~413, 1191, 1195, 1340, 1342, 1371) and
`http_route_preparse.rs:231`, `request_restriction.rs:375` reference
`crate::core::gateway::{lb, path_normalization, cli::config}` inline. Resolved by the
moves above (ctx → gateway; path_normalization relocation).

## Remediation strategy (summary)

1. **Move runtime out, keep schema in.** The recurring root cause: compiled runtime
   artifacts (`BackendSelector`, `PluginRuntime`, `DslPlugin`, `PluginConditions`) live as
   `#[serde(skip)]` fields inside `types` structs. Move them into gateway-side
   "prepared" wrappers; `types` keeps only the serializable config.
2. **Relocate genuinely-shared helpers downward** (`path_normalization`,
   `TcpKeepaliveConfig`, rate-limit shared constants) into `core::common` or `types`.
3. **Move `ctx.rs` wholesale into `core::gateway`** (gateway-only consumer).
4. **Delete the dead `core::controller::conf_mgr` re-export.**
5. **Add a guard** (extend `make check-agent-docs` or a dedicated CI grep / clippy
   `disallowed-*`) failing the build if `src/types/` references `core::gateway` or
   `core::controller`.

## Suggested phasing (subtasks)

1. ✅ **DONE (2026-06-11) — P0 trivial:** deleted Group D dead re-export
   (`types/mod.rs` `conf_mgr` aliases — confirmed unused).
2. ✅ **DONE (2026-06-11) — Move `ctx.rs` → `core::gateway::ctx`** (Group A). `git mv`
   (history preserved); removed `pub mod ctx;` + ctx re-exports from `types/mod.rs`;
   added `pub mod ctx;` + mirrored re-export in `core::gateway::mod.rs`. Migrated all
   consumers (all in `core/gateway`): `crate::types::ctx::*` → `crate::core::gateway::ctx::*`,
   and root re-exported ctx symbols (`EdgionHttpContext`, `Rsc`, `MatchInfo`, …) →
   `crate::core::gateway::*`. `cargo check --all-targets` + `cargo clippy` pass.
3. ✅ **DONE (2026-06-11) — Relocate helpers** (Groups B/C helper edges):
   - `core::gateway::path_normalization` → `core::common::path_normalization` (pure
     module, zero crate deps). Moved its global static + `init/get_path_normalization_config`
     out of `cli::config` into the module too.
   - `TcpKeepaliveConfig` (struct + `to_pingora`) → `types::resources::common`
     (`types` already depends on pingora via `err.rs`; `gateway::routes::common` now
     imports it back).
   - Rate-limit globals (`RateLimitGlobalConfig`, `GATEWAY_INSTANCE_COUNT`,
     `RATE_LIMIT_GLOBAL_CONFIG`, estimator-slot consts + getters, `init_rate_limit_global_config`)
     extracted from `gateway::cli::config` → new `core::common::rate_limit_config`.
     Redirected consumers (types `rate_limit.rs`, gateway `cli/{mod,config}.rs`,
     `conf_sync/conf_client/grpc_client.rs`). Removed now-unused `use tracing;` from cli/config.rs.
   - `cargo check --all-targets` + `cargo clippy` pass. `request_restriction.rs` and the
     path-norm part of `http_route_preparse.rs` are now fully clean.
4. **Peel route runtime** into `PreparedRoute` wrappers (Group B).
5. **Peel plugin runtime** incl. the DSL VM (Group C) — expect this to be the longest.
6. **Add the CI guard**; run full `cargo check --all-targets` + `cargo clippy` + tests.

> Remaining `crate::core::gateway` edges in `src/types/` after steps 1–2 (= steps 3–5):
> `resources/{http_route,http_route_preparse,grpc_route,tcp_route,tls_route,udp_route}.rs`,
> `resources/link_sys/webhook.rs`, `resources/edgion_plugins/{mod,entry}.rs`,
> `resources/edgion_plugins/plugin_configs/{dsl,rate_limit,request_restriction}.rs`,
> `resources/edgion_stream_plugins/mod.rs`. These compile today (single crate tolerates
> the cycle) and are the schema-vs-runtime peel work.

## Affected modules

| Module | Impact |
|--------|--------|
| `src/types/` | Source of all edits; becomes a clean leaf |
| `src/core/gateway/` | Receives `ctx`, prepared-route wrappers, compiled plugin/DSL runtime |
| `src/core/common/` | Receives relocated pure helpers (path normalization, configs) |
| CI (`Makefile` / `make check-agent-docs`) | New invariant guard |

## Risks

- **DSL VM coupling** (`plugin_configs/dsl.rs`) is the deepest edge; budget the most time.
- Route schema/runtime split touches hot data-plane structs — behavior must stay
  identical; rely on existing integration tests + `cargo test --all`.
- `#[serde(skip)]` field moves can ripple into many construction sites in gateway.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-06-11 | `types` must never depend on `core::gateway`/`core::controller` | Make `types` a portable leaf crate, shareable by Edgion + EdgionCenter |
| 2026-06-11 | Move `ctx.rs` into gateway rather than de-dep it in place | It is gateway runtime state; only gateway consumes it |
| 2026-06-11 | Delete `conf_mgr` re-export instead of relocating | Aliases are unused dead code |

## References
- `center-split-hub-management-plane.md` — why this decoupling is the prerequisite.
- `tasks/todo/main-bin-cargo-split/` — the eventual workspace/crate split.
