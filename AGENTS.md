# Edgion — AI Agent Project Guide

> ## ⚠️ READ BEFORE WRITING ANY CODE — [`skills/00-design-rules.md`](skills/00-design-rules.md)
>
> The **mandatory graded design-rules index** (🔴 MUST / 🟡 SHOULD / 🟢 PREFER): sensitive-data
> redaction, logging by binary, observability restraint, cross-cutting unification, reuse-first,
> failure policy, schema/transport invariants, workqueue discipline, time/duration, Rust safety,
> English-only. It is short and points to the detailed canon. **Scan its 🔴 rows against your
> diff before every commit.** Nothing below overrides a 🔴 rule there.

## Unit-test location

Put unit-test bodies in each production crate's
`tests/unit/<source path>/<stem>_test.rs` (`src/dns/mod.rs` maps to
`tests/unit/dns/mod_test.rs`). Keep a `#[cfg(test)] #[path = "..."] mod tests;`
declaration in the source module so tests retain private access and the existing
`cargo test --all` command and test names. Preserve other gates, ignored cases,
fixtures, and source-relative paths; compare test identities before and after a
move. Do not widen production visibility for tests. Top-level `tests/*.rs`
remain integration targets. Benchmarks live under `tests/benches/` with explicit
`[[bench]]` paths and `harness = false` in the owning crate's `Cargo.toml`. See
[`skills/05-testing/00-unit-testing.md`](skills/05-testing/00-unit-testing.md)
for naming exceptions and migration details. `edgion-tests` remains the
dedicated integration-test harness.

## Project Overview

Edgion is a Kubernetes gateway built on Rust + Pingora, using a Controller–Gateway split architecture with gRPC-based configuration sync.
It supports HTTP/1.1, HTTP/2, gRPC, TCP, UDP, TLS, and WebSocket, and includes a plugin system, load balancing, and TLS/mTLS.

### Product scope: proxy-layer gateways

Edgion focuses on two proxy-layer products: a general-purpose API Gateway and an AI Gateway for
LLM/provider traffic. The AI Gateway may understand provider request/response formats when that is
necessary for routing, credential isolation, quotas, resilience, guardrails, usage, or cost, but it
does not become an agent runtime or business orchestration platform.

Edgion does **not** currently implement MCP or A2A protocol gateways. Do not add MCP/A2A discovery,
session, tool, agent-card, protocol-routing, or protocol-translation behavior to the Controller,
Gateway, shared resource union, or ordinary HTTP routing path. If either protocol is pursued later,
treat it as a separately designed server/product with an explicit boundary; reuse Edgion's generic
proxy mechanisms only where their semantics genuinely match. Ordinary MCP/A2A HTTP traffic may
still pass through Edgion as opaque HTTP traffic using normal API Gateway features.

**Three product binary roles**: Controller (handles K8s resources), Gateway
(data-plane proxy), and edgion-cli (CLI debugging tool). The Gateway builds
exclusively against the Edgion Pingora fork (`Pandaala/pingora`, branch
`edgion_0.9.0_v1`). The federation Center has been extracted to the separate
EdgionCenter repository.

**Workspace layout** (Cargo workspace, resolver 2): the code is split across crates rather than one root crate —
- `edgion-algo` — runtime-independent matching and cache algorithms. Leaf crate with external dependencies only; it owns the path, IP, Gateway API Route hostname, and RFC 6125 certificate-hostname matchers plus the strict local/sharded LRU. Resources, Common, Controller, and Gateway consume these canonical implementations while retaining their policy and runtime orchestration.
- `edgion-waf` — runtime-independent native WAF engine. Leaf crate with no workspace-internal dependencies; it owns bounded SecLang/CRS parsing, compilation, transaction semantics, and multi-pattern matching. `edgion-resources` consumes its strict compiler for Controller/Gateway validation parity, and the Gateway consumes its transaction runtime. External WAF engines are references and differential-test oracles only, never production dependencies.
- `edgion-wasm` — runtime-independent Proxy-Wasm compatibility analyzer and exact Edgion host ABI profile. Leaf crate with external dependencies only; the Gateway consumes it before Wasmtime compilation and `edgion-cli` consumes it for local Wasm inspection.
- `edgion-dsl` — EdgionDSL language core: nom parser, bytecode compiler, static validator, and the config-independent compile/validate entry points (`resolve_script`, `check_redis_link_names`). Leaf crate with no workspace-internal deps. VM execution and builtin runtime dispatch stay in `edgion-gateway`.
- `edgion-resources` — shared resource/config schema. Depends only on runtime-independent leaves: `edgion-algo`, `edgion-dsl` (so `DslConfig` validation can run the real compiler), and `edgion-waf` (so native WAF generations receive the same strict validation in Controller and Gateway).
- `edgion-common` — shared infra: config loader, observability/log bootstrap, secret store, link-system core (+ etcd provider), TLS validation, auth, conf_sync/fed_sync proto codegen. Depends on `edgion-resources`. Backend-agnostic and does not link Pingora. Exposes a `test-util` feature for cross-crate test helpers.
- `edgion-gateway` — data plane; the `edgion-gateway` binary lives here. Owns the supported `boringssl` backend, the dormant future `rustls` backend feature, and allocator features.
- `edgion-controller` — control plane; the `edgion-controller` binary lives here. Depends only on `edgion-algo`/`edgion-common`/`edgion-resources` — **not** on `edgion-gateway`. Plugin validation on resource ingest runs the schema-layer `EdgionPlugins::validate_plugins` (in `edgion-resources`), which reproduces the gateway `compile_plugins` preparse errors in full — DSL and native WAF sources are compiled and statically validated via the shared leaf crates; the only gateway-only residual is DSL VM init (`Vm::new`), which cannot be predicted statically. The former `controller → gateway` edge is gone, so the workspace dependency graph is a clean DAG (`{algo, dsl, waf} → resources → common → {gateway, controller, cli}`, plus direct `algo → {common, gateway, controller}`, `waf → gateway`, and `wasm → {gateway, cli}` edges).
- `edgion-cli` — the `edgion-cli` binary + lib; depends on `edgion-common`/`edgion-resources` and the external-only `edgion-wasm` leaf.
- `edgion-tests` — the end-to-end integration-test harness; a workspace member kept out of `default-members`. Owns the test/helper bins (`test_server`, `test_client`, `cache_residue`, …). There is **no** root `edgion` package — the workspace root is a virtual manifest.

Each binary crate sets its own `#[global_allocator]`; libraries set none.
Because each binary is its own package, the precise form is
`cargo build -p <crate> --bin <name>`. The three main binaries
(`edgion-gateway`, `edgion-controller`, `edgion-cli`) are all in
`default-members`, so a bare `cargo build --bin <name>` from the workspace root
*does* resolve them. The bins
that do NOT resolve bare are the ones in `edgion-tests` (`test_server`,
`test_client`), since that crate is deliberately excluded from
`default-members` — build those with
`cargo build -p edgion-tests --bin <name>`.

**Feature Flags** (mutually exclusive — pay attention at build time):
- TLS backend: `boringssl` (default and only supported Gateway backend); the mutually exclusive
  `rustls` feature is a dormant future-development seam, not a supported build target
- Memory allocator: `allocator-jemalloc` (default) | `allocator-mimalloc` | `allocator-system`

### ACME scope

Built-in ACME supports HTTP-01 only. Do not add DNS-01, DNS provider adapters,
or DNS webhook clients without an explicitly authorized scope change. For DNS-01
or wildcard issuance, use cert-manager or another external issuer; Edgion already
loads and hot-reloads their standard TLS Secrets through its existing certificate
references. See [ACME service](skills/01-architecture/01-controller/08-acme-service.md).

### Gateway rustls boundary

BoringSSL is the only supported Gateway TLS backend. Do not implement rustls support
until Pingora's required TLS capabilities are complete and the user authorizes the
[pending task](tasks/pending/rustls-gateway-tls-support.md). Do not report or fix
rustls-only support gaps in ordinary reviews or cleanup. Backend-independent rustls
clients are unaffected. Details: [TLS backend policy](skills/01-architecture/02-gateway/04-tls/03-ssl-backends.md).

**BoringSSL FIPS (opt-in image build)**: `fips` is a gateway cargo feature enabled explicitly
through `cicd/build-image.sh` (requires `boringssl`; not a default, not enabled by the ordinary
release workflow, and not toggled at runtime). Boring 5 builds its bundled FIPS module source by
default. The canonical amd64/arm64 builder pins that same source revision and backports BoringSSL
commit `4d6b2f971e456962b70e4e91768caffdb36fe016`, which fixes the arm64 integrity-hash relocation;
the override is isolated to explicit FIPS builders and ordinary builds do not fetch it. The upstream
`BORING_BSSL_FIPS_PATH` prebuilt-tree escape hatch is outside the canonical script contract.
On a FIPS build,
downstream server TLS defaults to the FIPS-approved TLS 1.2 cipher set
(`ECDHE-{ECDSA,RSA}-AES{128,256}-GCM-SHA{256,384}`) and ECDH curves (`P-256`/`P-384`/`P-521`)
unless `EdgionTls.spec.ciphers` is set explicitly (operator override always wins). The gateway-local
`require_fips` config field (also settable via `EDGION_REQUIRE_FIPS`) fails startup non-zero if
`require_fips: true` but the binary is not a FIPS build. The gateway-local `fips_policy` field is a
tri-state (`auto` | `on` | `off`, default `auto`) resolved with precedence env
(`EDGION_FIPS_POLICY`) → CLI (`--fips-policy`) → config file → `auto`; there are no legacy booleans
and no legacy truthy/falsey tokens — a YAML boolean or an unrecognized env token is rejected
(the env case WARNs and falls through to the config instead of being treated as `off`, so a typo
can never silently disable enforcement). `auto` enables the approved cipher/curve policy when the
host kernel reports FIPS mode (`/proc/sys/crypto/fips_enabled`), which lets a FIPS node tighten the
profile without per-cluster configuration — this is cipher-suite policy alignment only and is NOT a
compliance claim. The module metric reports BoringSSL FIPS mode; it does not establish CMVP
certificate scope, artifact provenance, or deployment compliance. A non-FIPS binary on a FIPS node also emits a startup WARN;
alert on `edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0`. Known
limitations: TLS 1.3 cipher suites
cannot be restricted in BoringSSL (`TLS_CHACHA20_POLY1305_SHA256` still negotiable under FIPS), and
upstream (gateway→backend) TLS cipher suites are not FIPS-restricted (pingora connector
limitation). See `skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md` for the current
build and verification contract.

## Knowledge System

When a task needs project context, start from `skills/SKILL.md` and **load progressively on demand** — do not read everything at once. The detailed AI Agent collaboration guide lives in this file (`AGENTS.md`); `CLAUDE.md` is a symlink to it.

### Skills navigation rules

1. **Progressive loading**: `skills/SKILL.md` → category `SKILL.md` → specific files. Load only the smallest subtree the current task needs.
2. **Quick lookup first**: `skills/SKILL.md` has a quick-lookup table that maps keywords to the shortest path. Prefer it over walking the tree level by level.
3. **Three-layer lookup**: To understand architecture → `01-architecture/`; to look up a feature / configuration schema → `02-features/`; to write code → `01-architecture/` (development guides) + `03-coding/`.
4. **Resource-related tasks**: First go to `01-architecture/05-resources/SKILL.md` for the architecture documents, then to `02-features/` for the feature schema. You typically need to consult both sides.
5. **`docs/` is not a starting point**: `docs/` targets human users; `skills/` targets AI agents and developers.

### Review knowledge in everyday work

Before proposing or implementing a behavior change, design change, or refactor,
search [04-review](skills/04-review/SKILL.md) by owning function, behavior, and
symbols. Load only relevant pages, following the
[knowledge routing rules](skills/04-review/knowledge-routing.md).

Treat retrieved conclusions as evidence to verify, not proof that a finding is
false. Check their scope, assumptions, cited implementation, and re-evaluation
triggers against current code and relevant tests. Historical test results are not
fresh verification; a category or recorded verdict does not establish correctness.
Superseded guidance cannot suppress a current defect.

Distinguish intended requirements from observed behavior: current code may violate
a valid rule, and a knowledge page may be stale or wrong. Resolve conflicts using
the applicable project authority and concrete evidence; do not silently change a
required invariant or infer acceptance from existing code. If the conflict remains
unresolved, report it explicitly rather than closing the finding as a false positive.
Correct confirmed knowledge errors in the owning page within the task's scope;
track unrelated or unresolved discrepancies as follow-up work. Preserve useful
historical evidence without presenting it as current guidance.

Independent reviewers must read applicable repository rules and original knowledge
pages themselves, and verify both the implementation and the knowledge conclusion.
A parent-agent summary is a starting point, not a substitute for those sources.

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
# Full check (--workspace also covers `edgion-tests`, which is a workspace
# member but kept out of `default-members`; without it, check/clippy skip it)
cargo check --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --all
python3 cicd/checks/validate_agent_docs.py

# Targeted integration test
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>

# Full integration test
./edgion-tests/integration/scripts/integration/run_integration.sh
```

## Pre-commit checks (trigger phrases)

When the user says "run check", "check the code", "format the code", "run CI", "pre-commit check", or anything similar, run the following in order:

1. **Format**: `cargo fmt --all` (apply directly — not `--check`)
2. **Compile check**: `cargo check --workspace --all-targets`
3. **Lint**: `cargo clippy --workspace --all-targets`
4. **Doc consistency**: `python3 cicd/checks/validate_agent_docs.py`
5. **SSA force guard**: `python3 cicd/checks/check_ssa_force.py` — every `Patch::Apply` must use
   forced `PatchParams`; an unforced apply 409-Conflicts forever once any other field manager owns
   the object (a POST-created object, a `kubectl edit`, a Secret predating the controller). See the
   script's header for the full rationale and the allowlist rules.
6. **Metrics inventory guard**: `python3 cicd/checks/check_metrics_inventory.py` —
   the inventory doc must list every emitted metric (both directions) and no emit
   site may use a high-cardinality label off the allowlist.
7. **Pingora source policy**: `python3 cicd/checks/check_pingora_source_policy.py` —
   all seven Edgion fork dependencies must share the development `edgion_0.9.0_v1` branch
   or one release tag, and the lockfile must resolve the complete fork family
   to one commit.
8. **Gateway API transform contract**:
   `python3 cicd/checks/generate_gateway_api_transform_contract.py --check` — the
   committed RequestRedirect/URLRewrite OpenAPI, CEL, defaults, bounds, and scoped
   rule inventory must match both vendored Gateway API CRD channels.
9. **Unit-test layout**: `python3 cicd/checks/check_unit_test_layout.py` —
   enforces production crates and excludes the `edgion-tests` integration harness.

> `--workspace` is intentional: `edgion-tests` is a workspace member excluded from `default-members`, so a bare `--all-targets` would skip it. `--workspace` brings it back into the fast check/clippy signal (matching what `cargo test --all` already compiles).

Steps 2/3/4/5/6 may run in parallel. If any step fails, report the error immediately and try to fix it; after fixing, re-run that step. Once all pass, summarize the result.

> `cargo test --all` takes a long time and is not part of the quick check by default. Run it only when the user explicitly asks.

## Coding constraints

- Language: Rust; follow the project's existing style.
- Before committing, ensure `cargo check`, `cargo fmt --check`, and `cargo clippy` all pass.
- Log ID and observability conventions are in `skills/03-coding/`.
- Do not introduce security vulnerabilities (injection, XSS, and other OWASP Top 10).
- Keep changes minimal — do not refactor or add features that were not requested.
- **Pre-release: no backward-compatibility burden.** Edgion has not shipped to production yet, so a refactor requested by the user does not need to preserve historical behavior. Do not keep deprecated fields, legacy aliases, migration shims, or `#[deprecated]` re-exports "for compatibility", and do not add a compatibility layer nobody asked for. Rename/move/delete freely, update every call site and the docs, and let the compiler and tests be the safety net. This does not license *unrequested* refactors — the "keep changes minimal" rule above still holds.
- **Data-plane performance outranks mock convenience.** Never keep an owned copy, an extra allocation, or a slower shape on the request path just to stay easy to drive through `MockPluginSession`. If a site is awkward to mock, extract the pure logic and cover it with ordinary unit tests; keep the mock test for the wiring only.
- **Written content language**: everything written to disk must be English (code, comments, doc comments, TODO/FIXME, log/error literals, commit messages, `skills/` and `docs/en/` documentation). The only exception is Chinese user-facing documentation under `docs/zh-CN/`.
- **No smuggling past the English-only lint**: judge by runtime meaning, not source bytes. If a value's decoded form lands on a blocked codepoint, it's smuggling — whether spelled as `\u{...}`, YAML/JSON `\uXXXX`, percent-encoded UTF-8 (`%E4%B8%AD`, `%D1%80`, ...), runtime byte/hex/base64 construction, or `include_bytes!` / external file. Use an English equivalent that exercises the same code path: percent-decoding can be tested with a space (`hello world` ↔ `hello%20world`) or reserved char (`foo&bar` ↔ `foo%26bar`); multi-byte UTF-8 coverage with lint-permitted chars (`é`, `ñ`, `€`, `©`, `π`).

## Workqueue discipline (controller conf_sync)

The controller's per-kind processing model. These are rules, not descriptions —
code that violates them is wrong even if it "works":

1. Every resource kind has exactly **one** workqueue and **one** worker.
2. Dequeue processing is **serial** and must not wait.
3. **Every** runtime event — apply *and* delete — enqueues a key via
   `ResourceProcessor::on_event_key`. Nothing calls `process_resource`/
   `process_delete` directly. *Sanctioned exception*: the init phase
   (`on_init_apply` → `process_resource`) runs pre-worker on a single task,
   where the serial invariant holds trivially.
4. Anything needing reprocessing goes through **requeue** (never a direct call,
   never a second queue).
5. Handler processing must contain **no API calls, timers, sleeps, or other
   latency** — one slow item stalls every resource of that kind.

Initialization completion is idempotent within each resource-controller run:
FS, etcd, and Kubernetes ignore duplicate `InitDone` events before completion
callbacks or worker creation. Keep the first worker handle until shutdown; see
`skills/04-review/controller/regression-guards/single-worker-initialization.md`.

Deliberate, accepted exception to Rule 5:

- Status IO stays **inline** in the worker (FS file IO; a real API call in K8s
  mode). Moving it to an async status-writer / coalescing queue is **rejected**:
  it breaks the queue's ordering / read-your-writes invariant — the worker is the
  sole serial writer *and* reader of the FS `.status` file it compares against,
  and a deferred write would leave the persisted status trailing the config
  conf_sync has already shipped. Head-of-line latency is the accepted price of
  correctness, and it is self-limited: writes are gated on `status_changed`, so a
  steady-state relist of unchanged resources does zero writes and the loop is
  pure CPU. See
  `skills/04-review/handler-requeue/intentional-design/worker-path-status-io-inline-accepted.md`.

Because handlers must tolerate delete→create coalescing (the queue dedups by
key, so `on_delete` may never run for a recreated key), the apply path must
fully replace every side effect of the previous generation: registries are
clear-before-add, and dependents referenced only by the previous generation
are requeued (see `GatewayRouteIndex::update_route`'s dropped-keys return).

## CRD schema ↔ struct invariants (periodic inspection)

Resource structs in `edgion-resources/src/resources/` serve two paths: the **K8s CR path** (the hand-written CRD under `config/crd/edgion-crd/` is a *structural* schema, so the API server **prunes** any field the CRD does not declare — a silently-pruned field becomes a no-op with no admission error) and the **controller→gateway conf_sync path** (serde serializes the *full* struct, including controller-resolved values, to the data plane). This creates a standing invariant for every serialized field:

- **Operator-facing field** (a human sets it in a CR) → MUST appear in the CRD YAML. The guard below enforces fixed structural portions of all nine custom resources; deliberately opaque config/reference interiors remain review boundaries, as documented in the schema and transport contract.
- **Non-operator field** (controller-resolved host/port/listener, an internal cache, or a resolved **secret** carried to the data plane) → MUST carry `#[schemars(skip)]` so it never enters the CRD schema. Keep it serialized for conf_sync (do NOT use `#[serde(skip)]` if the gateway needs the value; use `#[serde(skip)]` only for pure in-process caches the gateway re-derives). A resolved **secret value** additionally uses the `crate::redact::{redact_opt, redact_required}` serde adapter so the admin `/configclient` output shows `"[redacted]"` while conf_sync still ships the real value. A resolver-only struct that holds secret material should NOT derive `JsonSchema` at all (defense-in-depth against a future schema-visible embedding).

**Automated guard:** `edgion-resources/tests/crd_coverage.rs` — run it manually via `./cicd/checks/check_crd_coverage.sh`, or it rides CI's `cargo test --all`. It compares generated fixed `spec` and `status` property names against every served version of all nine custom CRDs, both directions: Direction-1 = struct field missing from the CRD; Direction-2 = stale CRD field with no struct backing. An explicit inventory separates fixed fields from opaque variant-config and preserved-reference bodies; narrowly scoped preserved optional omissions have reasons and preservation gates. See `skills/01-architecture/05-resources/00-schema-transport-contract.md` for the exact scope.

**Limits to check by inspection (the guard does NOT):** it checks fixed field *presence*, not types/patterns/defaults or variant validity. Intentionally open config and reference interiors are outside its scope; inspect their `#[schemars(skip)]`, serde and redaction discipline manually. HTTP and Stream plugin stage-entry `type` fields are open strings, guarded by `edgion-resources/tests/plugin_crd_type_coverage.rs` and `stream_plugin_crd_type_coverage.rs`. New plugin names require no CRD whitelist change. Unknown types decode into schema-hidden `Unsupported` sentinels: Controller validation reports `Accepted=False`, and Gateway runtimes fail closed even for disabled or policy-masked unknown entries. Known configs and nested condition enums remain typed and validated. When adding or renaming a struct field on any resource, re-run the guard and re-check this invariant; audit `serialize`d secret/resolved fields for a missing `#[schemars(skip)]`.

## HTTP body handling invariants

These are architectural boundaries, not implementation suggestions:

AI request-body processing does not default to streaming: choose canonical
streaming observation or the existing request `BodyStorage` according to
whether the feature needs incremental bytes or one complete document.

1. **Only the proxied request body uses shared body storage.** Request plugins that require
   the complete body share the request-scoped `BodyStorage`; it may spill from
   memory to disk and is bounded by the gateway physical body cap. Do not add a
   response-side `BodyStorage` or another request-body cache.
2. **The main proxy upstream response streams by default.** Process it through
   Pingora's per-chunk response-body path, preserving backpressure and
   unbounded download/SSE behavior. Response-body plugins may inspect or
   transform the current chunk but must not accumulate unbounded state.
   Full-upstream-body APIs or modes (for example DSL `resp.body` or ExtProc
   response `BUFFERED`) must be rejected during configuration validation.
   A semantic response window is permitted only through the canonical
   `edgion_gateway::sse::window::RawWindow`: it must retain original slices
   under all independent hard bounds, resolve each generation explicitly as
   release/replace/discard, flush once at EOS, and never become full-response
   aggregation. Holding bytes is not itself backpressure; an async decision
   point provides backpressure only while its processor callback is awaited.
   The sole full-document exception is an explicitly selected non-streaming
   OpenAI Chat-to-Anthropic converter, owned by `ai/conversion/chat_to_anthropic`.
   It may aggregate one response in memory only under an independently reviewed
   design that enforces input/output byte caps while receiving/serializing,
   parsing-complexity limits, a conservative per-exchange memory budget and a
   process-wide concurrent-memory budget, existing deadlines, and release on
   success, failure or cancellation. A payload cap alone is not a heap bound.
   Conversion must finish before final downstream headers/body commit so an
   invalid, truncated or oversized conversion can return a fixed client-dialect
   502; budget admission failure uses the existing local failure contract.
   Transport/header deferral must be reviewed separately and leave ordinary HTTP,
   native AI and SSE paths unchanged. This permits no response `BodyStorage`,
   disk spool, full-response `RawWindow`, general plugin whole-response API, or
   usage-only aggregation. The bounded converter and separately reviewed transport/header seam are now
   implemented under
   [Task 03](tasks/todo/ai-gateway-product-completion/03-bounded-anthropic-json-conversion.md);
   its recorded acceptance does not relax these requirements or accept broader aggregation.
3. **A plugin-initiated outbound call is a separate bounded exchange.** Such a
   call may aggregate its response body only behind an explicit hard byte cap
   (normally a 1 MiB global/configured limit), enforced while streaming the
   response. Exceeding the cap fails the call; it must never silently truncate,
   fall back to an uncapped read, or change the main upstream response policy.
4. **The Gateway builds only against the Edgion Pingora fork.** The former
   `body-buffering`/`upstream-pingora` flavor duality is gone; the request-body
   capture seam is unconditionally compiled. Mock/plugin-session defaults still
   return `BodyReadError::Unimplemented`; bodyless reads remain empty.
5. **Resolve request-body policy before the request hot path.**
   `EdgionGatewayConfig.spec.requestBody` is the single runtime policy source
   for the switch, defaults, and business overrides. Parse and resolve it when
   publishing the immutable EdgionGatewayConfig runtime snapshot, then copy the
   resolved boolean and byte counts into each request context. The request path
   may perform one lock-free snapshot load and bounded lookup; it must never
   parse sizes or take a blocking lock.

## Cross-cutting unification rules

One canonical implementation per cross-cutting mechanism — violating any line below is a bug.
Details, verified consumer lists, and exemptions: `skills/01-architecture/00-common/05-unification-points.md`.

### In-process cache selection

Edgion currently has two shared in-process cache implementations. Choose by semantics, not preference:

- Use `edgion_algo::cache::ShardedLruCache` when a caller needs explicit removal/invalidation or strict replacement behavior.
- Use Pingora's unchanged `TinyUfo` through `TinyUfoTtlCache` only for high-cardinality, admission-friendly data where eviction is acceptable and no per-key deletion is required. Its currently approved use is positive OIDC introspection results.

Do not add another cache implementation or broaden TinyUFO usage without documenting why these two cannot satisfy the required semantics.

1. Outbound HTTP clients are built via `build_outbound_client` (`edgion-common/src/link_sys_core/http_client.rs`); a bare `reqwest::Client::builder()` outside tests is a bug.
2. Outbound TLS (CA/mTLS/verify) resolves through `resolve_outbound_tls` — including non-reqwest dialers (tonic/ldap3/fred/rskafka).
3. A new egress path with a config- or request-influenced destination uses `EgressResolver` (or at minimum `classify_ip`) at dial time, disables proxy-env pickup, and denies redirects unless per-hop re-checked.
4. Operator-configured DNS resolution goes through the shared `ResolverSpec` (`edgion-common/src/dns/`); no second resolver library.
5. Redis access goes through the LinkSys `RedisLinkClient`; rate-limit Lua logic extends the shared `redis_rate_limiter` core.
6. Key extraction uses `KeyGet`; IP allow/deny matching uses `edgion_algo::matcher::radix_ip`; client real IP comes from `real_ip_extractor` — never ad-hoc re-implementations.
7. Templated config fields use the shared `${type:name}` engine; security-sensitive fields (deadlines, fail-open switches, destinations) use the restricted variable allow-list that rejects client-controlled variables.
8. Plugins reject requests via `PluginRunningResult::ErrResponse`; `ErrTerminateRequest` only for inherently dynamic responses, written before returning.
9. Fail-close error bodies never leak internal detail (dependency errors, addresses, traces) — detail goes to `PluginLog` and access-log diagnostics (`EdgionErrorCode`).
10. Failure policy is frozen, not converging: the four existing fail-open/
    fail-close shapes are grandfathered; do not add a fifth. New runtime-
    dependency plugins follow the auth/non-auth rule (auth = fixed fail-close,
    no knob; non-auth = explicit knob, default close) in
    skills/01-architecture/02-gateway/12-edgion-plugin-dev.md.
11. Access policy is expressed as **allow/deny lists plus `defaultAction`**.
    Whitelist behaviour is implied by configuring an allow list (see
    `IpRestrictionConfig::allow_mode`); anything unmatched falls to
    `defaultAction`.
12. Two client IPs, never conflated. **`DirectPeerIp`** is the physical transport
    peer observed before inbound PROXY protocol or HTTP forwarded-header
    processing; it is never rewritten and is available wherever an inet peer
    exists. **`RemoteIp`** is the client identity resolved for the current layer:
    it may come from a trusted PROXY hop at L4 and from the trusted-proxy
    `RealIp`/XFF resolver at L7. It is only as trustworthy as those trust
    boundaries, so security rules should prefer `DirectPeerIp` unless the
    deployment genuinely terminates behind a trusted proxy. These map to Envoy's
    `direct_remote_ip` / `remote_ip`. `RealIp` names the resolution mechanism,
    not an address field. HTTP access-log fields use the canonical names
    `direct_peer_ip` and `remote_ip`; protocol-specific logs follow the same
    vocabulary.
13. Condition predicates reuse the shared `Condition` enum; a container field name must state how its elements combine. Do not add a plugin-local predicate enum, Include/Exclude, RestrictionSource, or wildcard values.

## Git constraints

- **Do not run `git commit` or `git push` on your own**: After completing code changes, report what changed and wait for the user's explicit instruction before committing.

## Authentication system

- **Controller behavior (token-only, Istio-style)**: **No auto-generation.** Admin API auth is a single pre-shared bearer token. The operator supplies it via (highest precedence first): (1) env var `EDGION_ADMIN_TOKEN` injected from a K8s Secret (`edgion-admin-credentials`, key `token`) via `secretKeyRef` (recommended for Kubernetes); (2) the `admin_token` config field in a Secret-mounted config file. A blank token = not configured. The token maps to an allow-all wildcard RBAC policy and survives reloads. When no token (and no `cli_tokens`) is configured, the server starts and returns 503 fail-close on all Admin API business routes with a startup WARN — no panic, no `exit(1)`. There is no username/password and no OIDC admin login. See `skills/02-features/02-config/03-auth-bootstrap.md` for the full behavior matrix and migration guide.
- **Scoped automation tokens**: configure role-limited tokens via `cli_tokens` (each with a fine-grained RBAC policy; hot-reloadable). See `skills/02-features/02-config/00-controller-config.md`.
- **View the K8s token**: `kubectl -n edgion-system get secret edgion-admin-credentials -o jsonpath='{.data.token}' | base64 -d`
- **API authentication**: Bearer token only (`Authorization: Bearer <token>`) — no cookie, no session.
- **CLI authentication**: `edgion-cli login --token <T>` persists the token to `~/.edgion/token`; token precedence `--token` → `EDGION_TOKEN` → `~/.edgion/token`. No username/password, no OIDC.
- **Authentication endpoints**: only public `GET /api/v1/auth/status` (`{ auth_required, mode: "token", status }`). The former `/api/v1/auth/login`, `/logout`, `/me` were removed.
- **Module locations**: `edgion-controller/src/api/cli_token_middleware.rs` (token authn), `edgion-controller/src/authz/mod.rs` (`Authorizer` + admin token), `edgion-controller/src/api/mod.rs` (onion + 503 gate + status handler), `edgion-controller/src/cli/mod.rs` (token resolution). The controller authz skip-set (canonical public paths) lives in `edgion-controller/src/api/public_paths.rs`. The gateway OIDC plugin's JWKS/discovery provider and JWT/JWK helpers live in `edgion-gateway/src/plugins/http/openid_connect/` (`jwks_provider.rs`, `jwt_common.rs`). `edgion-common` no longer carries any auth-specific module.
- **Configuration**: the `admin_token` field + `cli_tokens` list (see `skills/02-features/02-config/00-controller-config.md`).
- **Gateway data-plane auth plugins** (`openid_connect`, `jwt_auth`, `basic_auth`, `header_cert_auth`, ...) are unrelated to Admin API auth and are unchanged.

## Knowledge source rules

- `skills/` is the task-oriented knowledge layer (for AI agents and developers).
- `docs/` is the human-facing documentation layer.
- Do not duplicate detailed content between the two — pick one canonical source and link to it.
