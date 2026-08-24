# Edgion vs agentgateway — Feature-by-Feature Comparison

| Key | Value |
|-----|-------|
| Created | 2026-08-09 |
| Status | completed (baseline: agentgateway v1.4.1, 2026-07-29; Edgion `feature-06-24`) |
| Type | investigation / competitor analysis |
| Goal | Functional-direction parity (same capability surface) + efficiency parity (no bulk copies, streaming matching). Implementation mechanics deliberately differ. |
| Sources | agentgateway shallow clone read at source level (paths cited); agentgateway.dev docs/blogs; Edgion inventory in `01-data-plane-design-review.md` |

agentgateway: Rust on **tokio + hyper 1.9 + axum** (NOT Pingora, NOT Envoy),
Apache-2.0, Linux Foundation; single data-plane binary + optional Go control plane
speaking Envoy-xDS *transport* with purpose-built resource types; K8s Gateway API
v1.6 + `AgentgatewayBackend/Policy/Model` CRDs; ~monthly releases since v1.0
(2026-03). Edgion: Rust on the Edgion Pingora fork, Controller–Gateway split with
gRPC conf_sync, Gateway API + `Edgion*` CRDs.

Legend for the Edgion column: ✅ have (functional parity) · 🟡 partial · ❌ missing ·
➖ deliberately out of scope.

---

## 1. Architecture & platform

| Feature | agentgateway | Edgion | Gap notes |
|---|---|---|---|
| Data-plane language/stack | Rust, tokio/hyper/axum; jemalloc-mimalloc per platform | Rust, tokio/Pingora fork; same allocator options | ➖ equivalent, different stacks |
| Control plane | Go controller (istio.io libs), custom xDS types; also standalone YAML hot-reload; SQLite/Postgres config store (v1.4) | Rust controller, gRPC conf_sync, K8s/FS/etcd providers | ✅ equivalent capability, different protocol |
| Gateway API | v1.6; claims all-conformance on v1.3; ListenerSets, TCPRoute, Mesh profile | Gateway API supported; conformance status not formally tracked | 🟡 conformance benchmarking is a gap, not a feature gap |
| Config granularity philosophy | 1 user resource ↔ 1 xDS resource, O(change) churn | Per-kind workqueue + full-resource conf_sync; ArcSwap snapshots gateway-side | ➖ equivalent goals |
| Extensibility model | CEL everywhere (forked, optimized engine); ExtProc/ExtAuthz/RLS (Envoy protos); **no Wasm/Lua** | EdgionDSL (compiled bytecode VM); ExtProc; **full proxy-wasm host**; plugin system ~40 HTTP plugins | ✅ Edgion strictly richer (Wasm + DSL); CEL absent but DSL covers the niche |
| Ambient mesh / HBONE / waypoint | Yes (`crates/hbone`) | ❌ none | ➖ recommend out of scope (Istio-ecosystem-specific) |
| UDP proxy | ❌ none | ✅ has UDP | Edgion ahead |
| Built-in admin UI | Next.js UI with LLM/MCP cost dashboards | Separate edgion-dashboard project | ✅ equivalent (external) |

## 2. LLM gateway

| Feature | agentgateway | Edgion | Gap notes |
|---|---|---|---|
| Client-facing endpoint classes | chat completions, **/v1/messages (Anthropic-native clients)**, /v1/models, **/v1/responses**, /v1/embeddings, **/v1/realtime WS**, count_tokens, /v2/rerank, Passthrough, Detect (`crates/llm/src/lib.rs:85 RouteType`) | 🟡 OpenAI `/chat/completions` only (`ai_proxy/plugin.rs:130-132`) | **Major gap**: Anthropic-native ingress, /v1/models synthesis, embeddings, responses API, realtime WS |
| Providers (egress) | OpenAI, Anthropic, Gemini, Vertex, Bedrock (SigV4+event-stream), Azure (Entra), Copilot + 13 OpenAI-compat presets | 🟡 OpenAI, OpenAICompatible, Anthropic (`edgion_backend.rs:99-104`) | Gap: Bedrock/Vertex/Azure native signing & framing; presets are trivial for us (OpenAICompatible + endpoint) |
| Schema translation | N×M matrix incl. streaming variants (completions↔messages↔responses↔Converse↔Gemini) (`crates/llm/src/conversion/`) | 🟡 OpenAI↔Anthropic both directions, request + streaming response (`ai/anthropic/`) | Gap grows with providers; our per-pair translator design matches theirs |
| Model aliasing / routing | Wildcard aliases; **virtual models** (Weighted/Failover/CEL-Conditional); model-centric `AgentgatewayModel` CRD; header-based selection; PreRouting body-derived route selection | 🟡 alias→canonical per service (`edgion_backend.rs:106-222`); 5 backendSelection algorithms incl. CostWeighted/LowestLatency (richer than theirs); no virtual-model indirection; no /v1/models listing | Gap: model-level routing indirection & conditional routing; strength: adaptive latency/cost algorithms they lack |
| Failover / retries | Generic retry with body replay; virtual-model failover; EWMA locality LB; outlier detection | ✅ ≤7 retries/8 dispatches, credential-rotation-before-service-failover, credential outlier ejection, circuit breaker, adaptive telemetry (`ai/attempt.rs`, `ai/credential_outlier.rs`) | Edgion equal or ahead on resilience mechanics |
| Credential management | `BackendAuth` passthrough/key/OAuth; Azure/Bedrock credential caches; virtual API keys with metadata | 🟡 credential pools (≤16) with per-credential RPM/TPM Redis reservations + weights (`edgion_backend.rs:53-97`) — richer pooling; ❌ no OAuth-to-provider, no virtual API keys issued to clients | Gap: gateway-issued virtual keys; provider OAuth |
| Token rate limiting | Local token bucket `limit_type: tokens` + **tiktoken pre-tokenization** + post-response true-up; Envoy RLS remote | ✅ Redis tumbling-window token quota, two-phase admission + cumulative mid-stream reconciliation + **mid-stream terminate** (`ai/quota/`) — stronger enforcement; ❌ no real tokenizer (chars÷4 estimate, `dispatch.rs:479`) | Edgion enforcement is stronger (they cannot cut a stream); gap: accurate pre-count via tokenizer is optional/cheap to add |
| Quota exceeded actions | reject (429) | ✅ Reject / Throttle / Downgrade-model + mid-stream termination | Edgion ahead |
| Prompt guard | Regex + PII rulesets, webhook, OpenAI moderation, Bedrock Guardrails, Model Armor, Azure Content Safety; mask/reject; **streaming windowed guardrails with overlap tails**; realtime WS guarding (`llm/policy/`) | ❌ nothing LLM-content-specific today (WAF task is generic HTTP) | **Major gap**; windowed streaming evaluation needs the SSE zero-copy/window work (`todo/sse-decoder-zero-copy-stream.md` already cites agentgateway/NeMo prior art) |
| Prompt enrichment / body transforms | prepend/append messages; defaults/overrides/CEL transforms; prompt-caching marker injection | ❌ none (DSL can mutate request bodies generically, but no message-level vocabulary) | Gap: message-aware enrichment; caching-marker injection is cheap and high-value |
| Usage & cost | Normalized usage incl. reasoning/cache splits; **models.dev auto-refreshed price catalog**; `gen_ai_cost` metric; TTFT/TPOT; SQLite/Postgres analytics store; UI dashboards | 🟡 fixed-point micro-USD pricing on the CR, `estimated_cost_usd` in access log (ledger-first policy); TTFT recorded; ❌ no auto price catalog, no analytics DB (deliberate: access log is the ledger) | Functional parity on accounting; catalog auto-refresh optional; DB/UI ➖ (dashboard project's job) |
| Streaming usage extraction | Injects `stream_options.include_usage`, parses every SSE event; AWS event-stream decode | ✅ same injection (`ai_proxy/plugin.rs:632-643`) + per-event extraction + provider-authoritative totals | Parity (minus Bedrock framing) |
| Semantic caching / routing | Not built in — vLLM Semantic Router via ExtProc | ❌ ditto (our ExtProc can host the same integration once response streaming modes exist) | Parity of absence; their integration path = ExtProc |
| Inference Extension (EPP/InferencePool, llm-d) | Yes, via ExtProc InferenceRouting | ❌ | Gap only if K8s self-hosted-model routing is a target |
| LiteLLM import | Yes | ❌ | Nice-to-have migration tooling |
| Fault injection | Yes (delay/abort) | ✅ fault_injection plugin | Parity |

## 3. MCP gateway

Edgion today: **nothing MCP-specific** (generic HTTP/SSE proxying only). agentgateway
is the reference for the entire section (built on official `rmcp` SDK):

| Feature | agentgateway | Priority for Edgion |
|---|---|---|
| Streamable HTTP + legacy SSE downstream | `mcp/streamablehttp.rs`, `mcp/sse.rs` | P0 — table stakes |
| Upstream transports: stdio spawn, SSE, Streamable HTTP | `mcp/upstream/` | P0 (stdio P1 — K8s pod exec model differs) |
| Protocol version negotiation (through MCP 2026-07-28, stateless MCP, tasks) | `mcp/README.md`, `session.rs:1116` | P0 baseline version; chase-latest is continuous cost |
| Tool multiplexing/federation, name prefixing, merge fanout, failOpen/failClosed | `mcp/mergestream.rs`, `router.rs` | P0 — the core "MCP gateway" value |
| Session store: TTL, reaper, backend pinning (their CVE fix), persistence | `mcp/session.rs` | P0 — security-sensitive |
| Tool/prompt/resource-level RBAC (CEL) incl. filtered tool lists | `mcp/rbac.rs` | P0 (Edgion vocabulary: DSL or structured policy instead of CEL) |
| OAuth Protected Resource Metadata (RFC 9728), JWT modes, IDP presets, dynamic client registration passthrough | `mcp/auth.rs` | P1 (JWT reuse exists; PRM endpoint is new) |
| Token exchange / XAA / Entra OBO | examples, blog | P2 enterprise |
| MCP guardrails (per-method, external client) | `mcp/guardrails/` | P2 |
| OpenAPI→MCP tool generation | `mcp/upstream/openapi` | P2 |
| MCP Apps (UI meta rewrite) | `mcp/apps.rs` | P3 |
| MCP observability (method-labeled metrics, `_meta` trace propagation) | `telemetry/metrics.rs:122` | P1, alongside our restraint policy |
| Resource subscriptions across upstreams | `mcp/subscriptions.rs` | P2 |

## 4. A2A gateway

agentgateway's A2A layer is small (222 lines): JSON-RPC proxying + **agent-card
URL rewriting** (v0.2/v0.3/v1.0 card shapes, X-Forwarded-Proto-aware) +
method/outcome-labeled observability. No A2A-specific RBAC or payload transforms —
generic route policies do the rest.

Edgion: ❌ none today, but the ingredients (JSON body inspection on request via
BodyStorage, streaming response classification) exist. **This is the cheapest
headline feature to reach parity on** — card rewrite is a small buffered-JSON
transform on a bounded, non-streaming response; method observability is a
request-side JSON peek.

## 5. General gateway (context — mostly parity or Edgion ahead)

| Area | agentgateway | Edgion |
|---|---|---|
| AuthN/Z | JWT/OIDC/Basic/APIKey/ext_authz/CEL rulesets | ✅ jwt_auth, openid_connect, basic_auth, key_auth, hmac, ldap, forward_auth, header_cert_auth, casbin-style RBAC… richer |
| Traffic policies | timeout/retry/CORS/CSRF/rate limit/mirror/rewrite/direct response/compression/session affinity | ✅ equivalent plugin set |
| ExtProc | Envoy proto, **FULL_DUPLEX_STREAMED response bodies** | 🟡 request BUFFERED/STREAMED; response body modes rejected by invariant (`plugin_configs/ext_proc.rs:251-258`) — revisit only with a bounded-window story |
| TLS | term/mTLS/BackendTLS/passthrough; no ACME | ✅ + ACME — Edgion ahead |
| Observability | OTel traces/logs/metrics, `gen_ai_*` semconv, CEL log fields, lazy capture, DB logs | 🟡 access-log-first + restrained metrics (deliberate policy); gap: OTel GenAI semconv naming if interop matters |
| Protocols | H1/H2/gRPC/WS/TCP/TLS-passthrough/PROXY/CONNECT/HBONE; no UDP | ✅ H1/H2/gRPC/WS/TCP/UDP/TLS; no HBONE |

## 6. Efficiency comparison (the parity bar that matters)

Verified at source on both sides:

| Dimension | agentgateway | Edgion | Verdict |
|---|---|---|---|
| LLM request handling | **Full buffer in memory** before policy/translation; default cap 2 MiB (`crates/http/src/lib.rs:6 DEFAULT_BUFFER_LIMIT = 2_097_152`); "Unfortunately we buffer just due to how our interface works" (`llm/mod.rs:1598`) | Full buffer via BodyStorage with **memory→disk spill** at 128 KiB and 32 MiB physical cap; 2 memcpys total; replay is zero-copy slices | **Edgion ≥ parity** (bounded memory under load; they OOM-bound at cap×concurrency) |
| LLM non-streaming response | Fully buffered (`llm/mod.rs:2037`) | Bounded 1 MiB usage-extraction buffer only; body streams through | Edgion ahead on memory; note their buffering enables response transforms we can't do (CEL response.body) |
| SSE streaming, same-format (passthrough) | Original frames forwarded unmodified; **one side-copy into a `BytesMut` decode buffer** for usage parsing (`parse/passthrough.rs:91,204`) | Chunk `Bytes` moves untouched; decoder side-path copies data content ~2× + per-event `serde_json::Value` | **Rough parity**; both do exactly one "parse tax" pass; our decoder's byte-at-a-time `Vec` push + JSON `Value` is heavier per event than their codec — closed by the zero-copy decoder task |
| SSE streaming, translating | Decode→`serde_json` parse→translate→re-encode per event (`parse/transform.rs`) | Same shape (`anthropic/response.rs:356-378`) **plus a second decode pass** by the usage extractor (`sse/mod.rs:28-30`) | Gap: eliminate double decode (translator should emit events, not bytes) |
| Backpressure on streams | hyper body polling end to end | Pingora bounded task channel (4) + blocking downstream write | Parity |
| Mid-stream enforcement | ❌ cannot terminate a stream on quota (true-up after the fact, `AmendOnDrop`, `httpproxy.rs:694`) | ✅ typed terminate with clean wire ending | **Edgion ahead — keep this** |
| Streaming content matching | Windowed guardrail evaluation w/ overlap tails (bounded); no DFA matcher | `regex_automata` dense-DFA byte-streaming matcher with zero window buffer (request side only today) | Edgion's matcher is the better primitive; gap is exposing it on the **response** side + windowed text extraction for guards |
| Hot-path config | lazy CEL capture, arc-swap snapshots, interned strings | ArcSwap snapshots, pre-resolved policy copies in ctx | Parity |
| Copies, plain proxy fast path | hyper-native streaming | 2 ingest memcpys then pure moves (see design review §5.2) | Parity |
| Benchmarks | Self-published: 36.9k QPS / p50 0.83 ms / 28.8 MiB RSS vs LiteLLM (mock backend); no Envoy comparison | None published | Action: reproduce their harness once parity lands |

**Efficiency conclusions**: (1) our body architecture is already at-or-above their
efficiency bar — the one real deficit is the SSE decoder's per-event cost, which
the planned slice-chain decoder closes; (2) their single biggest architectural
weakness (unbounded-ish full-memory request buffering, no stream termination) is
our strength — do not regress it while adding features; (3) streaming guardrails
are the only feature where they have a bounded-state streaming mechanism we lack.

## 7. Gap summary → roadmap input

> Architecture update, 2026-08-12: the ranking below remains product-gap
> evidence, but it is no longer execution order. Backend-dependent work first
> goes through the `EdgionBackend` foundation in
> `../roadmap/00-backend-foundation.md`. In particular, A2A waits for
> `spec.a2a`, canonical selected-slot transport, typed policy attachment, and
> optional request-body observation; MCP uses `spec.mcp`, not a separate CRD.

Ranked by (user value × distance):

1. **MCP gateway** — entire subsystem missing (P0 core: transports, federation,
   sessions, RBAC; then auth metadata, guardrails).
2. **LLM ingress breadth** — Anthropic-native `/v1/messages` clients, `/v1/models`,
   embeddings; then providers (Bedrock/Vertex/Azure) behind the existing
   translator seam.
3. **Prompt guard / streaming guardrails** — regex+PII+webhook with windowed
   streaming evaluation; depends on the SSE zero-copy/window work.
4. **A2A** — small, high-visibility parity win (card rewrite + JSON-RPC
   observability).
5. **Model-routing indirection** — virtual models (weighted/failover/conditional),
   `/v1/models` synthesis, header-based selection.
6. **SSE decoder zero-copy + windowing** — efficiency parity enabler for 3 and the
   Anthropic double-decode fix (already a designed task).
7. **Prompt enrichment + caching-marker injection; tokenizer-accurate pre-count;
   models.dev catalog; LiteLLM import; OTel GenAI semconv naming** — smaller
   follow-ons.

Out of scope (recommend): HBONE/waypoint, MCP Apps (until stable), analytics
DB/UI in-gateway (dashboard owns it), semantic caching in-process (ExtProc
integration instead, matching their choice).

The phased plan lives in `../roadmap/`.
