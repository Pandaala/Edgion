# tasks/ai-gateway — AI Gateway Program

Umbrella for everything AI-gateway related: the deep design analysis, the
agentgateway comparison, the parity roadmap, and all previously scattered
task files on the body-buffering / streaming pipeline the AI gateway is built on.

## Layout

Everything AI-gateway related lives under this single directory. Four sections:

| Path | Content |
|------|---------|
| `README.md` · `AGENT-PLAYBOOK.zh-CN.md` | This index, plus the operator playbook (Chinese by request): one self-contained agent-launch prompt per step of the roadmap. |
| `analysis/` | The knowledge base. **01** verified deep review of the data plane (fork seams, BodyStorage, response pipeline, SSE decoder, `src/ai`, copy-count tables — every claim `file:line`-cited). **02** feature + efficiency comparison vs agentgateway v1.4.1 with the ranked gap list. **03** 15-product competitive absorption record incl. explicit rejections (check it before proposing a new feature). **04** cross-workspace reconciliation + the merge conflict recipe. **05** semantic-layer decision memo (tokenizer/vector unlock framework — not scheduled). **06** inference-aware-routing decision memo (GIE path A/B/C — verdict: default don't). |
| `roadmap/` | What we build next: `00-overview.md` (target architecture, workstream graph, closed decisions) + `01`–`07` per-workstream scoping plans. Expand a workstream into a feature task dir at kickoff per `skills/07-tasks/SKILL.md`. |
| `todo/` | Live open work: design notes and issues the roadmap depends on (SSE zero-copy design, response-body hardening D-items, ext_proc parity, keepalive, stream timeout bounds), plus `open-findings/` — the AI code-review batch with its still-open P2/P3 findings and INDEX. |
| `pending/` | Where a kicked-off workstream's feature task package lives while it is being designed/implemented (created on demand per `skills/07-tasks/SKILL.md`; moved to `archive/completed-tasks/` when shipped). Empty today. |
| `archive/` | Finished or superseded material, kept for provenance: `completed-tasks/` (AI-1b translation, AI-2 metering, AI-3 resilience, dsl-request-body-value, request-body-capability-followups, historical TODO-body-caching — all shipped) and `program-origin/` (the original AI-0..AI-7 program tree; its unique rationale was absorbed into `roadmap/`, see `analysis/04` §4). |

## Reading order for a new agent

1. `analysis/01-data-plane-design-review.md` — what exists and why.
2. `analysis/02-agentgateway-comparison.md` — where we stand vs the target.
3. `roadmap/00-overview.md` — what we build next and in which order.

## Status snapshot (2026-08-09)

- Shipped: request-body seam + BodyStorage; async response filter + terminate;
  SSE decoder v1; ai_proxy + EdgionAIService (OpenAI/OpenAICompatible/Anthropic);
  Anthropic translation; token quota with mid-stream termination; cost ledger;
  resilience/routing (AI-3 code shipped; its subtask checkboxes are stale).
- **WS-00 cross-workspace merge: DONE** (2026-08-09) — this workspace merged
  `origin/feature-06-24` (carrying the mid-stream quota-termination feature
  `b0c9db66`) into its 33 review-fix commits; merge commit `2b6e5b23`, pushed.
  The sibling ws2 workspace had cherry-picked the same fixes; that line is
  abandoned and its PPv2 work plus this planning set were moved here.
  **This workspace is the single main line.** Conflict recipe kept for future
  reference in `analysis/04-ws4-reconciliation.md` §3b.
- **Next**: WS-04 A2A quick win → WS-06 SSE v2 → WS-01 MCP (traffic-layer
  scope) → WS-02 ingress breadth (popular-three providers) → WS-03 guards →
  WS-05 model routing → WS-07 parity tail.
- AI-3 (resilience/routing) is **implemented and verified** — the live tracker
  (checked subtasks + G-verification results) was carried from ws4 into
  `archive/completed-tasks/ai-gateway-resilience-and-routing/` (HANDOFF.md + 07-changelog.md).
- All five roadmap decision points are closed (resolutions recorded at the end
  of `roadmap/00-overview.md`): traffic-layer-only MCP (no stdio, no MCP authz
  for now), bounded-window clarification instead of an invariant amendment,
  GPT/Claude/Gemini providers only, no comparative benchmarking.
