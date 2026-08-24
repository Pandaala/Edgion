# AI Gateway Program

This directory is the planning and delivery hub for Edgion's AI-native gateway
capabilities. The program now uses `EdgionBackend` as its backend architecture:
Kubernetes `Service` remains a direct standard Gateway API backend, while
`EdgionBackend` is the namespaced protocol-aware extension backend for AI,
A2A, external, and future reviewed target types.

The authoritative architecture program is
[`tasks/plan/edgion-backend-unification.md`](../plan/edgion-backend-unification.md).
Roadmap and pending-task documents here refine that program; they must not
contradict it.

Current cross-track execution status is owned by
[`tasks/plan/README.md`](../plan/README.md). It records the active ExtProc work,
the reduced WP2 redesign, the SSE substrate, and the later Guardrail unary
callout.

## Layout

| Path | Content |
|------|---------|
| `analysis/` | Verified implementation review, competitor comparison, absorbed product research, and architecture reconciliation records. `07-edgion-backend-reconciliation.md` maps the old baseline to the new target. Earlier documents may describe the currently shipped `EdgionBackend`; they are evidence, not the target contract. |
| `roadmap/` | Current target architecture and scoped workstreams. Start with `00-overview.md`, then `00-backend-foundation.md`. |
| `pending/` | Formal task packages that have entered analysis/design/implementation. The old standalone A2A design is retained here as a blocked predecessor and is respecified against the unified backend prerequisites. |
| `todo/` | Live capability and hardening debt that is not yet a scheduled feature workstream. |
| `archive/` | Completed or superseded task material kept only for provenance. Archived resource names are not active implementation instructions. |

## Reading order

1. `tasks/plan/README.md` — current cross-track execution order and status.
2. `tasks/plan/edgion-backend-unification.md` — architecture and phased migration history.
3. `roadmap/00-overview.md` — program dependency graph.
4. `roadmap/00-backend-foundation.md` — delivery plan for the unified backend foundation.
5. `analysis/01-data-plane-design-review.md` — verified current data-plane baseline.
6. `analysis/07-edgion-backend-reconciliation.md` — old conclusions mapped to the unified backend.
7. The selected workstream document and its formal task package under `pending/`.

## Status snapshot (2026-08-12)

- Shipped baseline: request `BodyStorage`, streaming main responses, SSE
  decoder v1, `AiProxy`, the current `EdgionBackend` AI backend, OpenAI /
  OpenAI-compatible / Anthropic dispatch, Anthropic translation, quotas,
  accounting, resilience, and adaptive routing.
- Architecture decision: replace `EdgionBackend` with the strict-union
  `EdgionBackend`, initially exposing only `spec.ai`. No compatibility CRD,
  alias, dual runtime path, or rolling mixed-version overlap will remain.
- Backend Foundation Phase 1 and ExtProc attached response streaming are
  accepted. The next formal task is SSE Decoder v2 under
  `tasks/pending/sse-decoder-v2/`.
- Backend WP2 is independently ready for a reduced-scope design review without
  selected-backend bounded exchange. Guardrail unary callout waits for SSE
  Decoder v2's canonical window handoff.
- A2A is no longer a standalone quick-win implementation. It resumes only
  after the reduced canonical runtime, typed policy support, `spec.a2a`,
  optional request-body observation, and a freshly reviewed Agent Card fetch
  transport exist. ExtProc is not that routed transport.
- Kubernetes `Service` remains directly routable and is never wrapped for
  ordinary traffic.

## Execution rule

Every implementation phase is a separate feature-profile task and follows
`skills/07-tasks/new_feature_work_flow.skill`: assessment, design, independent
design review, implementation, independent code review, test reinforcement,
and repository checks. No implementation starts directly from a roadmap file.
