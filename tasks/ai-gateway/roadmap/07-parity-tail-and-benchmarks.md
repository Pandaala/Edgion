# WS-07: Parity Tail

**Goal:** The remaining small parity items.

**Scope decision (user, 2026-08-09): no comparative benchmarking.** The
originally proposed A/B harness against agentgateway is dropped. Quality and
efficiency are assured by our own integration tests plus the architectural
invariants (bounded buffers, zero-copy streaming); each workstream's DoD is
its integration coverage, not a benchmark number.

## Items (each independently shippable, ordered by value)

### 7.1 AI integration-test coverage consolidation
- Ensure every WS lands with integration scenarios in the existing suite
  (`edgion-tests/integration/`): streaming passthrough, translation pairs,
  quota termination, MCP federation round trips, A2A card rewrite, guard
  reject/mask — extending the test_server mocks as each WS specifies.
- One sweep item: a token-rate-controlled SSE stream mode on the test_server
  OpenAI mock (useful for long-stream/stall scenarios across WS-03/06 tests,
  independent of any benchmarking).

### 7.2 Prompt enrichment + prompt-caching marker injection
- On `AiProxyConfig`: `promptEnrichment: {prepend: [messages], append:
  [messages]}` and `promptCaching: {inject: auto, minTokens: N}` (add
  `cache_control` markers on the Anthropic provider path; no-op elsewhere).
- Implementation point: `canonical_openai_request` post-parse hook in
  `ai_proxy/plugin.rs` (request BodyStorage mutation).

### 7.3 Tokenizer-accurate pre-count (optional accuracy upgrade)
- Feature-gated `tiktoken-rs` (or `tokenizers`) behind
  `EdgionAIService.spec.tokenization: {enabled, encoding}`; replaces the
  chars÷4 estimate in `dispatch.rs:479` for admission/pre-reservation only —
  settlement stays provider-authoritative. Binary-size and build-time impact
  assessed at kickoff; default off.

### 7.4 Price catalog auto-refresh
- Optional controller-side fetcher for a models.dev-compatible price catalog
  (outbound rules apply), merging into `EdgionAIService` pricing defaults with
  CR-explicit values always winning (operator-owns-config principle). Ships
  with a vendored snapshot; refresh is opt-in.

### 7.5 OTel GenAI semconv alignment
- Rename/alias the AI access-log fields and the (few) existing counters to
  `gen_ai.*` semconv names where a 1:1 mapping exists — doc + inventory-guard
  update, no new metrics.

### 7.6 LiteLLM config import (migration tooling)
- `edgion-cli ai import --from litellm <file>` generating `EdgionAIService` +
  `HTTPRoute` + `AiProxy` YAML. Pure CLI codegen, no gateway change.

### 7.7 ExtProc response streaming (deliberate re-evaluation, not a commitment)
- agentgateway supports Envoy FULL_DUPLEX_STREAMED response bodies; we reject
  response body modes by invariant. After WS-06's bounded window exists,
  re-evaluate exposing ExtProc response STREAMED with per-chunk exchange +
  bounded in-flight window. Requires its own design review against invariants;
  parked here so it is not forgotten (relates to
  `todo/ext-proc-envoy-parity.md`).

### 7.8 Backlog absorbed from ws4 AI-2 (each needs user opt-in before scheduling)
- Virtual API keys + org/team/user budget hierarchy (architecture options
  recorded in `../archive/program-origin/02-metering-and-cost.md` §5 — Redis+CRD lean).
- Unit-abstract metering for non-chat endpoints (audio seconds, image count —
  don't hardcode tokens into the metering data model; ibid. §7).
- Session/trace grouping fields in the AI access log (ibid. §8).

## Also fold in (existing debt from the analysis docs, cheap while touching)
- Dangling spec reference in `streaming_response_processor.rs:4`.
- Stale AI-3 subtask checkboxes in `archive/completed-tasks/ai-gateway-resilience-and-routing/`
  (mark shipped items, pointing at commits).
- Hardening backlog D6-D12 triage: schedule D6 (emit totals), D8 (read-only
  chain eos contract), D10 (keepalive on terminate) into the fork's next touch
  (WS-06 sequencing step 3 touches the same files).
