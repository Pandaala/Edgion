# WS-02: LLM Ingress Breadth & Providers

**Goal:** Accept more client dialects (Anthropic-native `/v1/messages`,
`/v1/models`, `/v1/embeddings`) and add the third popular provider (Gemini) —
collapsing to the existing canonical-OpenAI internal form.

**Scope decision (user, 2026-08-09): popular-three providers only.** GPT
(OpenAI, have) / Claude (Anthropic, have) / Gemini (new, API-key Gemini API
`generativelanguage.googleapis.com`). **Bedrock is out of the roadmap**
(SigV4 + AWS binary event-stream not worth the weight without demand); Azure
and Vertex-OAuth are deferred until demanded (Azure is a cheap URI/auth
mapping whenever needed; OpenAICompatible already covers many endpoints).

**Reference:** agentgateway `RouteType` (crates/llm/src/lib.rs:85) and
`conversion/` matrix.

**Today:** ingress is hard-gated to `POST */chat/completions`
(`ai_proxy/plugin.rs:130-132`); providers are OpenAI / OpenAICompatible /
Anthropic (`edgion_ai_service.rs:99-104`); translation exists only as
OpenAI-canonical → Anthropic (`ai/anthropic/`).

## Architecture

- **One canonical internal request** (the existing `canonical_openai_request`
  product) stays the pivot. New edge dialects translate *into* it at the plugin;
  new providers translate *out of* it at dispatch. Never add pairwise paths.
- New module `edgion-gateway/src/ai/ingress/` for edge dialects; provider
  translators join `ai/anthropic/` as sibling modules (`ai/vertex/`,
  `ai/azure/`, `ai/bedrock/`).
- `EdgionAIService.spec.provider` enum grows per provider; each provider module
  owns request mapping, response mapping (non-streaming + streaming), error
  normalization, and credential injection — the seams `dispatch.rs:330-455`
  already defines (model rewrite hook, URI mapping :719-738, processor
  installation :427-455).
- Endpoint-class routing: replace the single path check with a small
  `IngressDialect` classifier (path suffix → dialect), configured on
  `AiProxyConfig` (`endpoints:` allow-list, default `[chatCompletions]` for
  compatibility).

## File map

```
Create:
edgion-gateway/src/ai/ingress/mod.rs           # IngressDialect enum + classify()
edgion-gateway/src/ai/ingress/anthropic.rs     # /v1/messages -> canonical request;
                                               #   canonical/OpenAI stream -> Anthropic SSE
                                               #   (reverse of ai/anthropic/response.rs;
                                               #   reuse its event model)
edgion-gateway/src/ai/ingress/models.rs        # GET /v1/models synthesis from the
                                               #   route's candidate model set (local
                                               #   response; no upstream call)
edgion-gateway/src/ai/ingress/embeddings.rs    # /v1/embeddings passthrough class:
                                               #   canonical embeddings request (no
                                               #   translation phase 1 — OpenAI-shape
                                               #   providers only), usage extraction
edgion-gateway/src/ai/gemini/{request,response}.rs   # Gemini API generateContent /
                                               #   streamGenerateContent mapping
                                               #   (API-key auth; plain SSE stream)

Modify:
edgion-gateway/src/plugins/http/ai_proxy/plugin.rs   # endpoint gate -> dialect
    classifier; per-dialect body handling (models = no body)
edgion-gateway/src/ai/dispatch.rs              # provider match arms; processor
    selection per (ingress dialect × provider)
edgion-resources/src/resources/edgion_ai_service.rs  # provider variants + validation
edgion-resources/.../plugin_configs/ai_proxy.rs      # endpoints allow-list
edgion-tests test_server                       # mock Gemini endpoints;
                                               # Anthropic-native client scenarios
```

## Phases

### Phase 1 — Anthropic-native ingress (`/v1/messages`)
The symmetric counterpart of what we already have; highest client value.
1. Dialect classifier + `endpoints` config with validation.
2. Request: Anthropic Messages → canonical OpenAI (mirror the 13-field whitelist
   discipline of `anthropic/request.rs`, fail closed on unsupported fields).
3. Response: when the serving provider is OpenAI-shaped, translate the stream
   back to Anthropic SSE — a new `StreamingResponseProcessor` in the AI slot,
   same sealed-failure/bounded rules as `anthropic/response.rs`. When provider
   is Anthropic: **passthrough fast path** (skip both translators; usage
   extraction reads Anthropic events directly — extend `usage.rs` with an
   Anthropic-events mode instead of double-translating).
4. Quota/usage/cost must be dialect-agnostic (they already key off canonical
   units) — regression tests.

### Phase 2 — `/v1/models` + `/v1/embeddings`
1. Models synthesis (interacts with WS-05 aliasing; ship basic listing now).
2. Embeddings as a non-streaming bounded class (usage extraction only).

### Phase 3 — Gemini provider
`AiProvider::Gemini`: API-key credential (reuse `AiApiKeyResolver` slot),
generateContent/streamGenerateContent URI mapping, request/response JSON
translation pair + streaming SSE translation processor (same
sealed-failure/bounded rules as `ai/anthropic/response.rs`), error
normalization into `ai/error.rs`.

### Not in this roadmap (popular-three decision)
Bedrock (SigV4 + binary event-stream); Azure OpenAI and Vertex-OAuth (revisit
on demand — Azure is a small URI/auth mapping when wanted).

## Absorbed from the ws4 AI-1 program doc (design inputs)

Source: `../archive/program-origin/01-provider-abstraction.md` §7/§8:

- **Capability matrix for failover**: each (provider, model) candidate carries
  a capability set (streaming, tools, vision, …); failover skips candidates
  that cannot serve the request's required capabilities — never silently
  downgrade a request to make it fit.
- **Non-chat endpoint policy**: translate chat + embeddings only; other
  provider endpoints are either metered passthrough (where usage extraction is
  possible) or explicit reject — no untranslated best-effort forwarding of
  unknown endpoint classes.

## Invariant & efficiency notes

- All ingress translation happens on the already-buffered request body
  (BodyStorage) — no new caches.
- Response translation stays per-chunk in the AI processor slot; passthrough
  pairs must skip translation entirely (the agentgateway "Passthrough vs
  Transform" split — our comparison doc §6 shows parity requires this).
- The Anthropic↔Anthropic and OpenAI↔OpenAI same-dialect pairs must remain
  byte-passthrough with side-path usage extraction only (no re-serialization).
- Provider errors normalize to the fixed OpenAI-shaped table in `ai/error.rs`
  (extend the table; no internal detail leakage — rule 9).
