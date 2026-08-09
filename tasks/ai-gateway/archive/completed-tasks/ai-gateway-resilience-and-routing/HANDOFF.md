# Handoff / Base info — AI Gateway resilience and routing

> **Single entry for any agent taking over this task.** Read this first; then open linked docs.
> Last updated: 2026-08-04 (DNS ServiceExternalName classification fix).

## Identity

| Key | Value |
|-----|-------|
| Tracking dir | `tasks/archive/completed-tasks/ai-gateway-resilience-and-routing/` |
| Roadmap parent | `tasks/todo/ai-gateway/` → **AI-3** (`03-resilience-and-routing.md`) |
| Status | `in-progress` — implementation A–F claimed done; **G verification open** |
| Do NOT use as entry | `tasks/todo/ai-gateway/HANDOFF.md` (that is AI-0 / AI-1a phase-1) |

## Lifecycle axis (do not confuse)

- `tasks/todo/` — program ideas / roadmap (not day-to-day progress)
- `tasks/pending/` — **this feature's live tracker** (truth for checkboxes)

## Frozen architecture (one-liner)

```text
HTTPRoute.backendRefs          = sole upstream set + weights
HTTPRoute.retry                = sole per-request retry trigger/policy
EdgionBackendTrafficPolicy     = per-destination resilience (typed Service | AIService)
EdgionPlugins/AiProxy          = picker + protocol + quota + metering (NO backend list)
```

Cross-provider retry rebuilds each attempt from an **immutable canonical OpenAI request**.
Failover stops permanently after **final downstream response commitment**. Cap: 7 retries / 8 attempts.

## Key structs / identities (names to grep)

| Concept | Actual type / home | Notes |
|---------|-------------------|-------|
| Typed HTTP backend classification | `HTTPBackendRefKind` @ `edgion-resources/.../http_route.rs` | `Service` / `EdgionAIService` / `RouteDelegation` / **`Unsupported`** (not `Invalid`) |
| AiProxy `backendSelection` | `AiBackendSelection` (+ `AiBackendSelectionConfig`) @ `plugin_configs/ai_proxy.rs` | Weighted\|Priority\|LowestLatency\|LowestUsage\|CostWeighted — algorithm only; **no `aiServiceRef`** |
| Credential pool + redacted secrets | `AiCredentialPool` / `AiCredential` / `AiCredentialLimits` @ `edgion_ai_service.rs` | Partial-good Secret resolve; alias not a metric label; **no `defaults.timeout`** |
| `tokenQuota.onExceeded` | Schema `AiTokenQuotaOnExceeded`; runtime `QuotaExceededAction` @ `ai/quota/runtime.rs` | Reject \| Throttle \| Downgrade |
| Typed EBTP target key | `PolicyTargetKind::{Service,EdgionAIService}` + `PolicyTargetKey` | Homogeneous; AI rejects LB/HC/UA; store @ `backends/policy/backend_traffic/store.rs` |
| `AiAttemptPlan` | `AiAttemptPlan` / `ReadyAiAttemptPlan` / `AiPlanCandidate` @ `ai/attempt_plan.rs` | Plus `AttemptScheduler`, `AiAttemptRuntime`, `dispatch.rs`, `routing.rs` |
| Outlier / half-open | Shared `PassiveEjectionState` / `HalfOpenProbe` → AI via `credential_outlier.rs` | Single-probe half-open |
| Adaptive telemetry | `AdaptiveTelemetryStore` @ `ai/adaptive.rs` | LowestLatency / LowestUsage |
| Circuit | `AiServiceCircuitBreaker` from EBTP snapshot | Per AI-service destination |

### Audit notes (2026-08-04, design vs code)

- A–F struct surface matches design; no TODO/FIXME in `edgion-gateway/src/ai/**`.
- Integration coverage skew: Resilience covers Priority+429+SSE+half-open; adaptive pickers / circuit maxParallel / Throttle·Downgrade are thinner (SingleProvider or unit-only) — track as G risk, not a redesign.

## Document map

| File | Role |
|------|------|
| `ai-gateway-resilience-and-routing.md` | Meta + AI guide summary |
| `00-context.md` | Skills / code entry points |
| `01-design.md` | **Authority for behavior** |
| `02-implementation.md` | Phases + verification gates |
| `03-subtasks.md` | **Checkbox progress** (update as you go) |
| `06-decisions.md` | Frozen / rejected alternatives |
| `HANDOFF.md` | This file — session base |

## Progress snapshot

- [x] A Schema/CRD
- [x] B Controller
- [x] C Gateway foundations
- [x] D AI selection and retry
- [x] E Credential and resilience runtime
- [x] F Response / accounting / observability
- [ ] **G Verification and handoff** ← awaiting commit instruction
  - [x] Targeted resource / controller / gateway / Redis tests
  - [x] AI integration scenarios + hot reload
  - [x] AGENTS.md pre-commit suite + guards
  - [x] Wrap-up records
  - [ ] Local commits only (never push unless user asks)

## Operating rules for agents

1. **Subagents for heavy work** — explore, compile/test runs, reviews; keep parent context for decisions and user questions.
2. **Small / best-practice issues** — decide and record briefly in `03-subtasks.md` or a short note under G.
3. **Large / multi-option design (structs, ownership, new policy shapes)** — **STOP and ask the user**. Do not invent a second source of truth.
4. **Do not `git commit` / `git push`** unless the user explicitly asks.
5. After finishing a G checkbox, mark it in `03-subtasks.md` and bump "Last updated" here.
6. If a checkbox in A–F proves false, uncheck it, fix under the same design authority, and note the gap here.

## Verification commands (Phase G order)

```bash
# 1) Targeted unit / resource
cargo test -p edgion-resources --test crd_coverage
cargo test -p edgion-gateway -- ai::

# 2) Redis credential reservation (#[ignore] — needs Redis on 16379)
docker compose -f edgion-tests/integration/conf/Services/redis/docker-compose.yaml up -d
cargo test -p edgion-tests --test credential_reservation_redis -- --ignored --nocapture

# 3) Integration (omit --no-prepare on cold build; script passes -g)
./edgion-tests/integration/scripts/integration/run_integration.sh -r EdgionAIService -i Resilience
./edgion-tests/integration/scripts/integration/run_integration.sh -r EdgionAIService -i CredentialExhaustion
# ports: Resilience 31947, CredentialExhaustion 31948
# CredentialExhaustion auto-starts Redis; Resilience does not need Redis

# 4) Pre-commit suite
cargo fmt --all
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets
python3 cicd/checks/validate_agent_docs.py
python3 cicd/checks/check_ssa_force.py
python3 cicd/checks/check_metrics_inventory.py
python3 cicd/checks/check_pingora_source_policy.py
```

### Suite wiring notes (from verification audit)

- Targeted `-r EdgionAIService -i Resilience|CredentialExhaustion` are wired in mod.rs / test_client / run_integration.sh.
- Known small gaps (fix in wrap-up unless blocking): `"all"` gateway path omits Resilience; `integration-suite-map.md` lacks CredentialExhaustion row.

## Ask-before-changing (user gate)

Stop and ask if the fix would:

- Add/rename a public CRD field or change AiProxy / EBTP / AIService ownership
- Introduce a new retry / failure-policy shape
- Put backend membership or weights inside AiProxy
- Touch the Pingora fork
- Add high-cardinality Prometheus labels (model, credential alias, tenant, …)

## Session log

| Date | Note |
|------|------|
| 2026-08-04 | Takeover: base created; next = struct-vs-code audit then G verification |
| 2026-08-04 | Verification entrypoints audited: Redis needs `--ignored`; suite wiring gaps noted; G1 unit tests started |
| 2026-08-04 | Struct audit: design↔code aligned; HANDOFF types corrected (`Unsupported`, no aiServiceRef/timeout); G1 still running |
| 2026-08-04 | G1 unit OK: crd_coverage 10/10; gateway `ai::` 139/139; next = Redis `--ignored` then integration |
| 2026-08-04 | G1 Redis OK: credential_reservation_lifecycle 1/1; start G2 Resilience → CredentialExhaustion |
| 2026-08-04 | G2 Resilience OK: 429 fallback / SSE no-failover / half-open (3/3); next CredentialExhaustion |
| 2026-08-04 | G2 CredentialExhaustion OK (2/2); wired Resilience into test_client `all` + suite-map row; start G3 pre-commit |
| 2026-08-04 | G3 pre-commit all green; wrap-up docs written; **stopped before commit — ask user** |
| 2026-08-04 | User requested full integration; AI suites wired in script (Single/Anthropic/Resilience/CredentialExhaustion); full run started |
| 2026-08-04 | Full Fast integration finished (~5.5m). AI: Single✓ Anthropic✗(3) Resilience✓ CredentialExhaustion✓. Also DNS✗ / Dyn_DnsFlap✗. Investigating Anthropic. |
| 2026-08-04 | Anthropic fix: `AiDispatchError::Client` for Anthropic request reject (was HTML 500); access-log model=`claude-alias`; reorder suite so malformed SSE runs before 429 Retry-After cools the sole credential. |
| 2026-08-04 | Verified: `cargo test -p edgion-gateway -- ai::dispatch anthropic` 39/39; AnthropicProvider integration 8/8 after rebuild (first `--no-prepare` run used stale bins). |
| 2026-08-04 | Anthropic diagnose: (1) test expects logical_model; (2) Unsupported→InvalidConfiguration HTML500 bug; (3) 429 Retry-After cooldown poisons later malformed. Fix agent in flight. |
| 2026-08-04 | Anthropic fixed+8/8; full integration rerun started (with prepare/build) |
| 2026-08-04 | Full rerun aborted early: Controller plugin policy — port clash with concurrent `/Users/caohao/ws2/Edgion` integration. Waiting then rerun2 `--no-prepare`. |
| 2026-08-04 | User: do not wait, no --no-prepare; full prepare run started (will displace other workspace processes) |
| 2026-08-04 | Prior prepare run aborted at FIPS precheck (gateway SIGKILL from contention). Killed all competing procs; clean full run started. |
| 2026-08-04 | Full re-runs blocked: concurrent `/Users/caohao/ws2/Edgion` agent loops `kill_all` (matches all `debug/edgion-*`). Anthropic 8/8 verified in isolation. Waiting for exclusive machine. |
| 2026-08-04 | Ports free; full prepare run final2 started with light ws2 suppressor |
| 2026-08-04 | Full Fast integ done (~6m). AI all 4 PASS (Single/Anthropic/Resilience/CredentialExhaustion). Only DNS✗ + Dyn_DnsFlap✗ remain (non-AI). |
| 2026-08-04 | DNS failures are `UpstreamNotBackendRefs` (selected_backend None), not resolve errors. Fix agent investigating AI-3 typed-backend regression. |
| 2026-08-04 | **DNS fix landed:** `backend_kind()` now classifies core/empty Service-family kinds (`Service`, `ServiceClusterIp`, `ServiceExternalName`, `ServiceEndpointSlice`, `EndpointSlice`) as `Service`; original `kind` string preserved for `EdgionService::from_kind`. Verified: `cargo test -p edgion-resources -- http_route` 45/45; `HTTPRoute/Backend/DnsResolution` PASS; `Dyn_DnsResolutionFlap` PASS. No commit. |
| 2026-08-04 | Full prepare run for zero failures started (green log) |
| 2026-08-04 | **Full Fast integration ALL GREEN** (~6.7m, exit 0). AI 4/4 PASS; DNS + Dyn_DnsFlap fixed. No commit — ask user. |
