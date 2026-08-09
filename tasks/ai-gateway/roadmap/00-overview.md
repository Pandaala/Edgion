# AI Gateway Parity Roadmap — Overview

> **For agentic workers:** each workstream file (01-07) is a scoping plan. At
> kickoff, expand the chosen workstream into a full feature-profile task dir
> (per `skills/07-tasks/SKILL.md`, using `new_feature_work_flow.skill`) with
> TDD-grade steps, using the workstream file as the frozen scope + file map.
> Do not implement from this overview alone.

**Goal:** Reach functional parity with agentgateway v1.4 (MCP gateway, A2A
gateway, LLM ingress breadth, prompt guard) while keeping Edgion's efficiency
edge (bounded memory, zero-copy streaming, mid-stream enforcement).

**Baseline documents:** `../analysis/01-data-plane-design-review.md` (what we
have, verified), `../analysis/02-agentgateway-comparison.md` (gap list §7),
`../analysis/03-competitive-absorption.md` (15-product research + recorded
rejections, inherited from ws4 — check it before proposing any new feature to
avoid relitigating), `../analysis/04-ws4-reconciliation.md` (ws4 status/code
delta). The origin program docs (AI-0..AI-7, with unique rationale prose) are
archived verbatim under `../archive/program-origin/`.

## Global constraints (apply to every workstream)

- CLAUDE.md HTTP body invariants 1-5 hold as written. Bounded hold-and-release
  of response content is already within invariant 2's letter (it bans
  *unbounded* state; the Anthropic translator's partial-event withholding is
  the shipped precedent). WS-06 adds a canonical clarification note so review
  language is settled; any window must be bounded, policy-declared,
  eos-flushed, and default-off.
- Cross-cutting rules 1-10 (outbound clients via `build_outbound_client`,
  `EgressResolver`, `KeyGet`, `ip_radix_tree`, templated-config engine, error
  redaction, frozen failure-policy shapes) apply to all new MCP/A2A/LLM code.
- New operator surface = new CRDs / plugin configs in `edgion-resources`,
  validated controller-side via the same schema-layer path as existing plugins;
  CRD coverage guards updated when structural.
- Observability restraint: access log + EdgionStatus first; metrics only via the
  inventory-guard process.
- No `git commit`/`push` without explicit user instruction; all on-disk content
  English.

## Target architecture (end state)

```
                      ┌────────────────────────────────────────────────┐
 clients              │ edgion-gateway                                 │   upstreams
                      │                                                │
 OpenAI dialect ────▶ │ ai_proxy plugin ──▶ src/ai (routing/quota/     │ ─▶ OpenAI/Anthropic/
 Anthropic dialect ─▶ │   + ingress dialect layer (WS-02)              │    Bedrock/Vertex/Azure
                      │   + guard pipeline (WS-03, windowed, WS-06)    │
                      │                                                │
 MCP clients ───────▶ │ mcp service (WS-01, terminating endpoint:      │ ─▶ MCP servers
  (streamable HTTP/   │   session store · federation router · tool     │    (streamable HTTP /
   legacy SSE)        │   filtering · bounded upstream exchanges)      │     SSE; stdio via
                      │                                                │     sidecar adapters)
                      │                                                │
 A2A clients ───────▶ │ a2a plugin (WS-04: card rewrite as bounded     │ ─▶ agent backends
                      │   outbound exchange + JSON-RPC observability)  │
                      │                                                │
                      │ shared: BodyStorage (req) · SseDecoder v2      │
                      │   (WS-06, slice-chain + windows) · quota ·     │
                      │   StreamingResponseProcessor · model router    │
                      │   (WS-05)                                      │
                      └────────────────────────────────────────────────┘
```

Key architectural decisions (locked here, details per workstream):

1. **MCP is a terminating service, not a transparent filter.** The gateway
   speaks MCP to clients and MCP to upstreams (agentgateway does the same).
   Upstream calls are plugin-initiated bounded exchanges (invariant 3) through
   `build_outbound_client`; the main-proxy streaming path is used only for the
   client-facing SSE/streamable-HTTP leg. This keeps invariants intact — no
   response-side BodyStorage is ever needed.
2. **A2A card rewrite is a gateway-initiated bounded fetch**, not a buffered
   proxied response: the gateway fetches `/.well-known/agent*.json` from the
   backend itself (1 MiB cap), rewrites, and serves it as a local response.
   JSON-RPC request observability uses the existing request BodyStorage.
3. **LLM ingress dialects normalize at the edge**: every client dialect
   (`/v1/messages`, `/v1/embeddings`, …) converts to the existing canonical
   OpenAI request before `AiAttemptPlan`, reusing `ai/anthropic/request.rs`
   patterns in reverse. One canonical internal form, N edge dialects, M
   provider dialects — same N×M-collapse agentgateway uses.
4. **Guardrails ride the existing `StreamingResponseProcessor` slot** for the
   response side and the body gate / `RequestBodyStreamHandler` for the request
   side; windowing state comes from SseDecoder v2 (WS-06). No new pipeline.
5. **The DFA streaming matcher (`StreamingRegexScanner`) is the shared matching
   primitive** for request and response guards — agentgateway has no equivalent;
   this is our efficiency edge, generalized in WS-06.

## Prerequisite: WS-00 — ws4 code reconciliation (BLOCKS everything)

The sibling workspace ws4 (/Volumes/ExtStore/ws4/Edgion, same branch name,
diverged from ancestor `d88d573f`) carries **20 review-fix commits + one
in-flight fix (gw-07) that ws2 lacks**, including real bug/security fixes
(AI-backend ReferenceGrant enforcement, Anthropic pause_turn/tool-choice-none,
partial-credential status, streaming compression stall guard, monotonic usage
accounting). Full commit table, conflict analysis, and cherry-pick strategy:
`../analysis/04-ws4-reconciliation.md` §3. **Do this merge before starting any
workstream** — WS-02/03/06 all touch the same `ai/` files and would compound
the conflicts. The open review findings carried into
`tasks/ai-gateway/todo/open-findings/` are re-triaged as part of this step.

## Workstreams and dependency order

| WS | File | Scope | Depends on | Size |
|----|------|-------|-----------|------|
| 01 | `01-mcp-gateway.md` | MCP terminating service, traffic-layer scope: transports, sessions, federation, static tool filtering (no stdio, no MCP-specific authz) | — (parallel-safe) | L |
| 02 | `02-llm-ingress-breadth.md` | `/v1/messages` ingress, `/v1/models`, embeddings; Gemini provider (popular-three policy) | — | M |
| 03 | `03-prompt-guard-streaming.md` | Request+response guards: regex/PII/webhook, mask/reject, windowed streaming | 06 (windows) for response side; request side independent | L |
| 04 | `04-a2a-gateway.md` | A2A card rewrite + JSON-RPC observability | — | S |
| 05 | `05-model-routing-virtualization.md` | Virtual models, conditional routing, `/v1/models` synthesis, header selection | 02 (models listing) partially | M |
| 06 | `06-sse-decoder-v2.md` | Slice-chain zero-copy decoder, hold-and-release windows, invariant amendment, Anthropic single-decode | — (unblocks 03) | M |
| 07 | `07-parity-tail-and-benchmarks.md` | tokenizer pre-count, prompt enrichment + cache markers, price catalog, LiteLLM import, OTel GenAI naming, benchmark harness | 02/05 for some items | M |

Recommended execution order: **04 (quick win) → 06 → 01 (longest pole, start
early; can overlap 06) → 02 → 03 → 05 → 07**. 01 and 02/06 can run in parallel
by different agents — they share no files except `edgion-resources` registration
points (coordinate via separate resource files).

Out of scope for the whole roadmap (per comparison doc): HBONE/waypoint, MCP
Apps, in-gateway analytics DB/UI, in-process semantic caching (ExtProc
integration instead), UDP-side changes.

## Decision points

Resolved (user, 2026-08-09):

1. ~~WS-01 stdio transport~~ — **permanently out of scope.** Edgion is a
   traffic-layer gateway; process execution belongs to sandboxes/runtimes.
   stdio MCP servers are bridged via sidecar/mcp-proxy adapters (documented).
2. ~~WS-01 RBAC vocabulary~~ — **MCP-specific identity/authorization deferred
   entirely** until Edgion has a real authorization-system foundation. Route
   authentication = existing generic auth plugins; traffic-level static
   per-member tool allow/deny lists cover the exposure-control need.

3. ~~WS-03/06 invariant-2 amendment~~ — **dissolved on re-analysis (2026-08-09)**.
   Invariant 2 bans *unbounded* state and full-body modes; bounded
   hold-and-release is already within its letter, and the shipped Anthropic
   translator already withholds partial-event bytes (bounded 1 MiB) across
   chunks (`ai/anthropic/response.rs:351` `chunk.take()` + decoder pending).
   Remaining work is a canonical clarification note in CLAUDE.md/skills (so
   reviewers have agreed wording) + the shared window primitive in WS-06.
   Chunk-driven stall behavior (held window waits for next chunk/eos when the
   upstream stalls) is analyzed as acceptable; a fork-level max-hold-time tick
   is optional P3.
4. ~~WS-02 provider order~~ — **resolved: popular-three only.** GPT (have),
   Claude (have), Gemini (new, API-key Gemini API). Bedrock is out of the
   roadmap; Azure and Vertex-OAuth deferred until demanded.

5. ~~WS-07 benchmark methodology~~ — **resolved (user, 2026-08-09): no
   comparative benchmarking.** No A/B harness against agentgateway; quality is
   assured by our own integration tests. The working principle: keep the code
   architecturally sound (bounded buffers, zero-copy paths, streaming
   invariants) and performance follows. WS-07 drops the benchmark item; each
   workstream's DoD is its integration-test coverage.

All five decision points are closed.
