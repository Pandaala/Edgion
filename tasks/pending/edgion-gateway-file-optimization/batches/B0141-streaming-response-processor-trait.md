# B0141 — Streaming Response Processor Trait

## Scope

- Baseline: `230c934675029ce69fefdaa7eabae08c6420fd1a`; revalidated at `99a94a73`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/traits/streaming_response_processor.rs`
- Baseline blob: `acc0103d7f973149c3e2c7e480cc81a0ba505a50`
- Proposed exact supporting allow-list:
  - `edgion-gateway/src/ai/anthropic/response.rs`
  - `edgion-gateway/src/ai/dispatch.rs`
  - `edgion-gateway/src/ai/usage.rs`
  - `edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs`
  - `edgion-gateway/src/plugins/http/guardrail/response.rs`
  - `edgion-gateway/src/response_processor_driver.rs`
  - `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs`
  - `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs`

## Handoff

- Analysis: `b0139_request_filter_analysis` audited the complete trait, driver/lifecycle, all 22 implementations, AI/ExtProc/Guardrail/RawWindow consumers, terminal/trailer behavior, tests and streaming knowledge. Decision: change; simple dead-API and documentation cleanup.
- Value: required `StreamingResponseProcessor::name` has zero callers across all five production and seventeen test implementations. Remove it and all stubs. Repair the target module's nonexistent design link and name the current Gateway lifecycle plus `SseDecoder` framing/`RawWindow` semantic retention contract.
- Preserved contracts: typed event and legacy bridge behavior, serial per-chunk awaits/backpressure, bounded buffers/windows, TerminalBeforeTrailers ownership, termination/preemption, PluginLog/access-log finalization, main response streaming and no response BodyStorage.
- Rejected expansion: removing the legacy async bridge or changing composite typed events lacks the required production profile/current defect evidence and remains unchanged.
- Review: pending promotion, implementation and fresh final review.
- Verification: focused response driver, ExtProc, Guardrail, AI composite and proxy response tests plus full gates.
- Commit: pending local batch commit; never push.
