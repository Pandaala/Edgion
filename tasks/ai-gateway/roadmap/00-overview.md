# AI Gateway Roadmap — Unified Backend Architecture

> Every workstream is a scoping plan. At kickoff, create or refresh a formal
> feature-profile task and follow `skills/07-tasks/new_feature_work_flow.skill`.
> Do not implement directly from this overview.

## Goal

Build AI and A2A gateway capabilities on one backend architecture while
preserving Edgion's bounded-memory and streaming guarantees.

The authoritative architecture is
`tasks/plan/edgion-backend-unification.md`. The core decision is:

- Kubernetes `Service` remains a direct standard backend.
- `EdgionBackend` is the only Edgion protocol-aware extension backend.
- `EdgionBackend.spec` is a strict exactly-one union, starting with `ai` and
  growing only through reviewed protocol variants.
- Frontend route behavior remains separate from backend identity.

## Target architecture

```text
clients
  |
  +-- OpenAI / Anthropic / Gemini dialects
  |      -> HTTPRoute + AiProxy
  |      -> selected EdgionBackend.spec.ai
  |
  +-- A2A
  |      -> HTTPRoute + A2aGateway frontend policy
  |      -> selected EdgionBackend.spec.a2a
shared backend pipeline
  -> typed weighted logical slots
  -> select once
  -> canonical target resolution
  -> typed TLS/traffic policy
  -> canonical peer and accounting
  -> ordinary streaming proxy

external response-processing pipeline
  -> ExtProc request-scoped gRPC Process stream
  or Guardrail bounded HTTP decision per canonical SSE window
```

## Global constraints

- Preserve all HTTP body invariants: only proxied request bodies may use
  BodyStorage; ordinary upstream responses remain streaming; plugin callouts
  are separate, explicitly bounded, and never masquerade as routed backends.
- One canonical route selection, Service/EndpointSlice resolver, DNS layer,
  egress classifier, outbound TLS resolver, peer builder, policy arbitration,
  and retry/accounting path.
- Backend variants own identity and destination data, not cross-cutting policy.
- Policy attaches to the directly routed logical backend and does not inherit
  implicitly through a wrapped Service.
- Raw forwarded scheme/host is untrusted without a canonical trusted-proxy
  mechanism.
- No new failure-policy shape, high-cardinality protocol metric, secret/body
  logging, topology leakage, or bare outbound client.
- New resource fields obey structural CRD, schemars/serde, redaction, strict
  union, ReferenceGrant, dependency-requeue, status, and hot-reload rules.
- No commit or push without explicit user authorization.

## Workstreams and dependencies

| WS | File | Scope | Hard dependency |
|----|------|-------|-----------------|
| 00 | `00-backend-foundation.md` | Replace AI resource; canonical selection/transport; typed policy; A2A variant; optional body observer | first priority |
| 02 | `02-llm-ingress-breadth.md` | Anthropic ingress, models, embeddings, Gemini | WS-00 F1 |
| 03 | `03-prompt-guard-streaming.md` | Request/response guards and bounded windows | WS-00 F1; WS-06 for response side |
| 04 | `04-a2a-gateway.md` | `spec.a2a`, Agent Card rewrite, request method observation | WS-00 F2-F5 plus fresh Agent Card fetch design |
| 05 | `05-model-routing-virtualization.md` | Virtual models and model-level candidate routing | WS-00 F1; WS-02 partially |
| 06 | `06-sse-decoder-v2.md` | Slice-chain decoder, shared windows, Anthropic single decode | independent; coordinate AI files with F1 |
| 07 | `07-parity-tail-and-benchmarks.md` | Small AI parity and tooling items | WS-00 F1; item-specific dependencies |

External response processing has a separate execution track under
`tasks/plan/stream-processing/`: ExtProc provides the stateful high-frequency
gRPC attached stream, while Guardrail unary callout provides one bounded HTTP
decision per larger SSE window. ExtProc, WS-06, and Guardrail Unary are
complete. The active follow-up is Work Package 17 composition: provider-aware
Replace/Reject framing, terminal precedence, and combined processor lifecycle.
Neither transport is a backend-selection mechanism.

## Recommended execution order

```text
WS-00 F0/F1: EdgionBackend.spec.ai replacement [complete]
  -> Stream Processing 01: ExtProc attached response stream [complete]
       +-> WS-00 F2: reduced canonical backend-runtime rescope
       |     -> WS-00 F3: typed policy capabilities
       |            -> WS-00 F4/F5 and later backend work
       `-> WS-06: canonical SSE decoder/window [complete]
              -> Stream Processing 02: Guardrail unary callout [complete]
                     -> WS-03 streaming response guard composition [next]

WS-02 ingress breadth, WS-05 model virtualization, and item-specific WS-07
work follow their actual file-ownership and backend prerequisites.

WS-04 A2A requires a fresh Agent Card fetch design; ExtProc is not its routed
transport.
```

The immediate active task is Stream Processing Work Package 1. The canonical
program status and cross-track gates are indexed in `tasks/plan/README.md`.

## Frozen decisions

1. `EdgionBackend` is removed, not deprecated; its fields move under
   `EdgionBackend.spec.ai`.
2. Resource kind wire number `23` is renamed in place. Migration is a
   maintenance-window stop-the-world cutover with no mixed-version support.
3. A2A backend identity lives in `spec.a2a`; Agent Card public origin and
   method observation remain a route-attached frontend plugin.
4. A protocol plugin requires homogeneous compatible positive-weight backend
   variants. Existing wrong variants reject with `Accepted=False`; missing or
   denied references retain invalid weight and report `ResolvedRefs=False`.
5. The former plugin-facing selected-backend bounded exchange and shared
   Pingora connector are withdrawn. ExtProc and Guardrail are plugin dependency
   transports and never rerun or participate in route weighting.
6. Optional body observation is a generic, independently proven prerequisite,
   not a special mandatory capture mode inside A2A.
7. Provider scope remains OpenAI, Anthropic, and Gemini; Bedrock and cloud OAuth
   providers wait for demand.
8. No comparative benchmark program; correctness, boundedness, structural
    efficiency, and integration coverage are the gates.

## Program completion

- `EdgionBackend` is the only Edgion extension backend resource.
- AI behavior is preserved on `spec.ai` with no old-resource residue.
- Service and extension backends share selection, transport, policy, retry,
  TLS, endpoint, accounting, and hot-reload mechanics.
- A2A card rewriting and method observation use `spec.a2a`, a freshly reviewed
  Agent Card fetch transport, and the observer foundation. ExtProc is not that
  routed-backend transport.
- Ordinary upstream responses remain streaming and all phase-specific tests,
  reviews, documentation, and repository checks pass.
