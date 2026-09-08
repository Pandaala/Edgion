# B0187 — HTTP Response Head

## Scope

- Analysis baseline: `ec774be0545717b7816622590cc755d1ba9abc2d`
- Target blob: `7b6134aff4e947ce2c00c2fd9e19871e8f52d07f`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Target: `edgion-gateway/src/routes/http/proxy_http/pg_response_head.rs`
- Existing `edgion-tests/**` files and protected user paths remain frozen.

## Handoff

- Analysis: examined-no-change. The target is already a compact allocation-free adapter around Pingora's bounded response-head barrier. Cache exclusion, Hold/Release/Replace ordering, terminal writer claim, 101 handling, framing, RawWindow composition, and canonical Guardrail observability match the current contracts.
- Verification baseline: target tests 5/5 and Guardrail tests 20/20 passed; frozen H1/H2 integration evidence was reviewed without editing it.
- Deferred candidate: Guardrail admission and final-head gating inspect only the first Content-Encoding field. Correct all-field/all-token classification requires a shared borrowed multi-value session seam and coordinated plugin/final-head changes, so it is wider than this batch.
- This is a queued candidate and cannot enter the serial mutation/review lane before earlier batches commit.
