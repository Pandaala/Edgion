# WS-03: Prompt Guard & Streaming Guardrails

**Goal:** Request + response content guards for LLM traffic: regex/PII rulesets,
external webhook, mask/reject actions — with **windowed streaming evaluation**
on responses (bounded hold-and-release), matching agentgateway's `llm/policy/`
capability using our stronger primitives (DFA streaming matcher + mid-stream
terminate).

**Depends on:** WS-00 F1 for the stable `EdgionBackend.spec.ai` identity and
WS-06 for response-side windows. Request-side design can start earlier, but
implementation must not overlap the repository-wide backend rename in the same
AI/plugin files. Note: bounded hold-and-release is already within invariant 2's
letter (shipped precedent: the Anthropic translator's partial-event
withholding) — WS-06 supplies the shared primitive and settled review wording,
not a rule change.

**Current transport schedule:** Stream Processing Work Package 1 (ExtProc) is
complete. WS-06 and Stream Processing Work Package 2 have also delivered the
canonical window and unary HTTP Pass/Replace/Reject callout. The active
response-side package is
`tasks/pending/streaming-response-guards/`: it composes those mechanisms and
must not implement another ExtProc client, Guardrail plugin, decoder/window, or
response-window HTTP transport.

The architecture sketch below predates the delivered `Guardrail` plugin. Its
request-side `AiGuard` regex/PII model remains future Work Package 16 scope.
For the current response package, Guardrail `Replace` is the external mask/
rewrite contract and Guardrail `Reject` is the external policy decision; only
provider-aware terminal framing, precedence, and composition gaps are in
scope.

## Architecture

- New plugin `ai_guard` (`EdgionPlugin::AiGuard`), attachable alongside
  `ai_proxy` on the same route (validated: requires ai_proxy present so message
  structure is known).
- **Request side**: runs at request stage over the buffered canonical body
  (BodyStorage view). Extracts message text spans (role-filtered), evaluates
  rules, then Allow / Reject (OpenAI-shaped 4xx via `ai/error.rs`) / Mask
  (BodyStorage mutation API — `replace_slice` already exists and revalidates
  caps).
- **Response side**: a `StreamingResponseProcessor` installed after the
  translation/usage processors. Consumes decoded content deltas (WS-06 event
  stream), maintains a bounded hold-and-release **text window** (target
  ~200-256 tokens with ~25% overlap — the prior art recorded in
  `todo/sse-decoder-zero-copy-stream.md`), releases chunks downstream once the
  window clears evaluation, and on match: Reject ⇒ `sink.push(error event)` +
  `sink.terminate()` (the existing quota-termination machinery — reuse
  `terminate_ai_stream`'s SSE-boundary discipline); Mask ⇒ rewrite the withheld
  window before release.
- **Rule engines**:
  - Regex rules: compile to the shared `regex_automata` dense DFA and stream
    bytes through it (`StreamingRegexScanner` generalized by WS-06); no
    backtracking engine on the hot path. Rules requiring captures/lookaround are
    evaluated per released window (bounded) with the standard `regex` crate.
  - PII ruleset: curated regex pack shipped as data (start from agentgateway's
    `policy/pii/` category list for coverage parity; our own patterns).
  - Unary external guard: use the separately scheduled Guardrail unary-callout
    work package at `tasks/plan/stream-processing/02-guardrail-unary-callout.md`:
    one bounded HTTP moderation/webhook decision per held window, with the
    canonical outbound client/TLS/egress path and the existing non-auth failure
    shape defaulting close.
  - Stateful high-frequency external processing is not another webhook mode;
    it is the existing ExtProc plugin extended by
    `tasks/plan/stream-processing/01-ext-proc-attached-response-stream.md`.
- **Moderation providers** (OpenAI moderation / Bedrock Guardrails / Model
  Armor / Azure Content Safety): same webhook seam with provider adapters;
  phase 3, one adapter each, gated on demand.

## File map

```
Create:
edgion-gateway/src/plugins/http/ai_guard/mod.rs
edgion-gateway/src/plugins/http/ai_guard/plugin.rs      # request-stage entry, config
edgion-gateway/src/plugins/http/ai_guard/rules.rs       # rule model: matcher kind,
                                                        #   scope (request|response),
                                                        #   action (allow|mask|reject),
                                                        #   per-rule failure policy
edgion-gateway/src/plugins/http/ai_guard/request_eval.rs  # span extraction from
                                                        #   canonical messages + eval
edgion-gateway/src/plugins/http/ai_guard/response_eval.rs # StreamingResponseProcessor:
                                                        #   window mgmt + release/mask/
                                                        #   terminate
stream-processing WP2 owned module(s)                   # bounded response-window HTTP decision
edgion-gateway/src/plugins/http/ai_guard/pii.rs         # builtin ruleset registry
edgion-resources/.../plugin_configs/ai_guard.rs         # config schema + validation
                                                        #   (window size caps, rule
                                                        #   count caps, regex compile
                                                        #   validation at admission)

Modify:
edgion-gateway/src/ai/dispatch.rs        # install response_eval into the AI
                                         #   processor chain (after usage/translation)
edgion-resources/.../edgion_plugin.rs    # variant + BodyRequirement (request stage,
                                         #   fail-close)
CLAUDE.md + skills invariant docs        # clarification note ships with WS-06, not here
edgion-tests                             # scenarios: request reject/mask, streaming
                                         #   reject mid-stream (wire-clean terminate),
                                         #   streaming mask across chunk boundary,
                                         #   webhook timeout fail-close/open
```

## Phases

1. **Request-side guard** (no dependencies): rules model + regex/PII + webhook +
   mask/reject on the buffered request. Deliverable: block/mask prompts.
2. **Response-side reject** (needs WS-06 windows): windowed evaluation with
   terminate-on-match; DFA rules only.
3. **Response-side mask + moderation adapters**: withhold-rewrite-release; then
   provider adapters behind the webhook seam.

## Absorbed from the ws4 AI-4 program doc (design inputs)

Source: `../archive/program-origin/04-guardrails.md`:

- **Structured refusal mode**: besides plain HTTP-4xx Reject, support HTTP 200
  with `finish_reason: "content_filter"` (streaming: a delta + stop event pair)
  so agent chains degrade gracefully instead of crashing on an error status.
  Config: `rejectionMode: error | contentFilter`.
- **Evaluator contract over vendor plugins**: the webhook seam is the single
  external-evaluator interface (Bedrock ApplyGuardrail-style); vendor
  moderation providers are adapters behind it, never separate plugins.
- **Parallel guard execution**: independent guards run concurrently with
  per-guard timeouts; chain latency = max, not sum.
- **PII-aware data-residency routing** (later phase): per-rule action
  `block | mask | route-to <backend>`; the target is a compatible typed
  `EdgionBackend.spec.ai` reference compiled through the canonical logical
  backend path. An unreachable or wrong-variant target is fail-close — never
  rerun route weighting or mask-and-send-external as a fallback.

## Notes

- Latency budget: window release adds at most one window of buffered delay to
  the stream; window size is operator-capped and validated (default small).
  Document the trade-off in the plugin docs; guards default off.
- The guard is not auth: failure policy is an explicit knob, default close
  (rule 10). Reject bodies use fixed OpenAI-shaped errors (rule 9).
- Realtime WebSocket guarding (agentgateway parity) is explicitly deferred —
  WS traffic bypasses the response-body pipeline today; revisit after WS-01/02.
