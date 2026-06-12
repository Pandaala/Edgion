# Edgion — AI Agent Project Guide

## Project Overview

Edgion is a Kubernetes gateway built on Rust + Pingora, using a Controller–Gateway split architecture with gRPC-based configuration sync.
It supports HTTP/1.1, HTTP/2, gRPC, TCP, UDP, TLS, and WebSocket, and includes a plugin system, load balancing, and TLS/mTLS.

**Three binaries**: Controller (handles K8s resources), Gateway (data-plane proxy), edgion-cli (CLI debugging tool). The federation Center has been extracted to the separate EdgionCenter repository.

**Workspace layout** (Cargo workspace, resolver 2): the code is split across crates rather than one root crate —
- `edgion-resources` — shared resource/config schema (leaf).
- `edgion-common` — shared infra: config loader, observability/log bootstrap, secret store, link-system core (+ etcd provider), TLS validation, auth, conf_sync/fed_sync proto codegen. Depends on `edgion-resources`. Backend-agnostic (TLS backend flows in via workspace `pingora-core` feature unification). Exposes a `test-util` feature for cross-crate test helpers.
- `edgion-gateway` — data plane; the `edgion-gateway` binary lives here. Owns `boringssl`/`rustls` + allocator features.
- `edgion-controller` — control plane; the `edgion-controller` binary lives here. Depends only on `edgion-common`/`edgion-resources` — **not** on `edgion-gateway`. Plugin validation on resource ingest runs the schema-layer `EdgionPlugins::validate_plugins` (in `edgion-resources`), which reproduces the gateway `compile_plugins` preparse errors minus the DSL deep checks (Rhai compile / VM init, gateway-only). The former `controller → gateway` edge is gone, so the workspace dependency graph is a clean DAG (`resources → common → {gateway, controller, cli}`).
- `edgion-cli` — the `edgion-cli` binary + lib; depends only on `edgion-common`/`edgion-resources`.
- root `edgion` package — a thin umbrella that only hosts integration-test `examples/` and the `cache_residue` helper bin.

Each binary crate sets its own `#[global_allocator]`; libraries set none. Because each binary is its own package, build them with `cargo build -p <crate> --bin <name>` (a bare `--bin <name>` from the workspace root no longer resolves).

**Feature Flags** (mutually exclusive — pay attention at build time):
- TLS backend: `boringssl` (default) | `rustls`
- Memory allocator: `allocator-jemalloc` (default) | `allocator-mimalloc` | `allocator-system`

## Knowledge System

When a task needs project context, start from `skills/SKILL.md` and **load progressively on demand** — do not read everything at once. The detailed AI Agent collaboration guide lives in this file (`AGENTS.md`); `CLAUDE.md` is a symlink to it.

### Skills navigation rules

1. **Progressive loading**: `skills/SKILL.md` → category `SKILL.md` → specific files. Load only the smallest subtree the current task needs.
2. **Quick lookup first**: `skills/SKILL.md` has a quick-lookup table that maps keywords to the shortest path. Prefer it over walking the tree level by level.
3. **Three-layer lookup**: To understand architecture → `01-architecture/`; to look up a feature / configuration schema → `02-features/`; to write code → `01-architecture/` (development guides) + `03-coding/`.
4. **Resource-related tasks**: First go to `01-architecture/05-resources/SKILL.md` for the architecture documents, then to `02-features/` for the feature schema. You typically need to consult both sides.
5. **`docs/` is not a starting point**: `docs/` targets human users; `skills/` targets AI agents and developers.

### Task lifecycle

The task templates, lifecycle stages, stage→Skills mapping, and trimming rules are all consolidated in `skills/07-tasks/SKILL.md`.

## Common workflows

- **New feature (needs architecture context)**:
  1. `skills/SKILL.md` → `skills/01-architecture/SKILL.md` → load only the directly related architecture files
  2. `skills/02-features/SKILL.md` (feature / configuration schema reference)
  3. `skills/05-testing/SKILL.md` (verification)

- **Add a new resource type**:
  1. `skills/01-architecture/01-controller/09-add-new-resource/00-guide.md`
  2. Pick the closest pattern reference (`route-like`, `controller-only`, `plugin-like`, `cluster-scoped`)
  3. `skills/01-architecture/00-common/03-resource-system.md`
  4. `skills/01-architecture/01-controller/03-config-center/SKILL.md`
  5. `skills/05-testing/01-integration-testing.md`

- **Add / debug an HTTP plugin**:
  1. `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`
  2. `skills/03-coding/observability/00-access-log.md`
  3. `skills/05-testing/01-integration-testing.md` (includes the "dynamic (hot reload) testing" section — if your plugin supports hot reload, add a dynamic scenario)

- **Add / debug a Stream plugin**:
  1. `skills/01-architecture/02-gateway/13-stream-plugin-dev.md`
  2. `skills/02-features/05-annotations/00-annotations-overview.md`
  3. `skills/05-testing/01-integration-testing.md` (includes the "dynamic (hot reload) testing" section — a stream scenario template is not yet provided; you can extend the HTTP scenario as a reference)

- **Debug routing / TLS / sync issues**:
  1. `skills/05-testing/SKILL.md`
  2. Controller restart / Gateway log `Unknown kind` → `skills/01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md`
  3. Controller↔Gateway gRPC sync → `skills/01-architecture/03-controller-gateway-link/SKILL.md`
  4. Gateway API semantics → `skills/08-kubernetes/01-gateway-api/SKILL.md`
  5. TLS gateway routing → `skills/09-misc/debugging-tls-gateway.md`

- **Understand configuration and path behavior**:
  1. `skills/02-features/02-config/SKILL.md`
  2. Load the configuration schema file for Controller, Gateway, or `EdgionGatewayConfig` as needed
  3. Relative path behavior → `docs/en/dev-guide/work-directory.md`

- **Modify `edgion.io/*` annotations**:
  1. `skills/02-features/05-annotations/00-annotations-overview.md`
  2. Load the corresponding `metadata.annotations`, `options`, or reserved / test-only key references
  3. Update stale examples; do not forward-port legacy keys

- **CI / release automation**:
  1. `skills/09-misc/cicd/02-github-workflow.md`
  2. `skills/09-misc/cicd/00-local-build.md`
  3. `skills/09-misc/cicd/01-docker-build.md`

## Common commands

```bash
# Full check
cargo check --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets
cargo test --all
python3 cicd/checks/validate_agent_docs.py

# Targeted integration test
./edgion-tests/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>

# Full integration test
./edgion-tests/scripts/integration/run_integration.sh
```

## Pre-commit checks (trigger phrases)

When the user says "run check", "check the code", "format the code", "run CI", "pre-commit check", or anything similar, run the following in order:

1. **Format**: `cargo fmt --all` (apply directly — not `--check`)
2. **Compile check**: `cargo check --all-targets`
3. **Lint**: `cargo clippy --all-targets`
4. **Doc consistency**: `python3 cicd/checks/validate_agent_docs.py`

Steps 2/3/4 may run in parallel. If any step fails, report the error immediately and try to fix it; after fixing, re-run that step. Once all pass, summarize the result.

> `cargo test --all` takes a long time and is not part of the quick check by default. Run it only when the user explicitly asks.

## Coding constraints

- Language: Rust; follow the project's existing style.
- Before committing, ensure `cargo check`, `cargo fmt --check`, and `cargo clippy` all pass.
- Log ID and observability conventions are in `skills/03-coding/`.
- Do not introduce security vulnerabilities (injection, XSS, and other OWASP Top 10).
- Keep changes minimal — do not refactor or add features that were not requested.
- **Written content language**: everything written to disk must be English (code, comments, doc comments, TODO/FIXME, log/error literals, commit messages, `skills/` and `docs/en/` documentation). The only exception is Chinese user-facing documentation under `docs/zh-CN/`.
- **No smuggling past the English-only lint**: judge by runtime meaning, not source bytes. If a value's decoded form lands on a blocked codepoint, it's smuggling — whether spelled as `\u{...}`, YAML/JSON `\uXXXX`, percent-encoded UTF-8 (`%E4%B8%AD`, `%D1%80`, ...), runtime byte/hex/base64 construction, or `include_bytes!` / external file. Use an English equivalent that exercises the same code path: percent-decoding can be tested with a space (`hello world` ↔ `hello%20world`) or reserved char (`foo&bar` ↔ `foo%26bar`); multi-byte UTF-8 coverage with lint-permitted chars (`é`, `ñ`, `€`, `©`, `π`).

## Git constraints

- **Do not run `git commit` or `git push` on your own**: After completing code changes, report what changed and wait for the user's explicit instruction before committing.

## Authentication system

- **Controller default behavior**: **No auto-generation.** The operator must supply credentials via one of three sources (highest precedence first): (1) env vars `EDGION_ADMIN_USERNAME` / `EDGION_ADMIN_PASSWORD` / `EDGION_ADMIN_JWT_SECRET` injected from a K8s Secret via `secretKeyRef` (recommended for Kubernetes); (2) `local_auth` in a Secret-mounted config file; (3) `auth` OIDC. When nothing valid is configured, the server starts and returns 503 fail-close on all Admin API business routes with a startup WARN — no panic, no `exit(1)`. See `skills/02-features/02-config/03-auth-bootstrap.md` for the full behavior matrix and migration guide.
- **Center default behavior**: **No auto-generation**. The operator must provide either `local_auth` or `auth`. If neither is provided, the server still starts; `/health`, `/ready`, `/metrics`, and `/api/v1/auth/status` remain accessible, but all business routes reject with 503 (fail-close via `initializing_response`, not panic/exit). See `skills/02-features/02-config/03-auth-bootstrap.md` for details.
- **View the K8s password**: `kubectl -n edgion-system get secret edgion-admin-credentials -o jsonpath='{.data.password}' | base64 -d`
- **API authentication**: Bearer token or httpOnly Cookie.
- **CLI authentication**: `edgion-cli login`, or auto-login by detecting the configuration file.
- **Authentication endpoints**: `/api/v1/auth/login`, `/api/v1/auth/logout`, `/api/v1/auth/status`, `/api/v1/auth/me`.
- **Module locations**: `src/core/common/local_auth/`, `src/core/common/unified_auth/`, `src/core/common/api/compose.rs`.
- **Configuration**: the `local_auth` YAML section (optional; see `skills/02-features/02-config/00-controller-config.md`).

## Knowledge source rules

- `skills/` is the task-oriented knowledge layer (for AI agents and developers).
- `docs/` is the human-facing documentation layer.
- Do not duplicate detailed content between the two — pick one canonical source and link to it.
