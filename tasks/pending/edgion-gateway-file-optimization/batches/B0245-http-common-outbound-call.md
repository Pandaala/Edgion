# B0245 — HTTP Common Outbound Call

## Scope

- Pipeline state: analysis-ready; must be revalidated against current HEAD before promotion.
- Active anchor: `edgion-gateway`.
- Baseline: `8fc76e83`.
- Target: `edgion-gateway/src/plugins/http/common/outbound/call.rs`.
- Target blob: `de92374aa0d00e84c4cb4966734427f7249e6095`.
- Inspect the complete bounded outbound-call lifecycle, timeout/body/framing/error behavior, consumers, tests and relevant review precedent.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected.

## Analysis Outcome

- Decision: change, complex/plan-first. The no-retry default still clones the fully built request before its sole execution; transfer ownership directly for one attempt while retaining current cloneability rejection and per-attempt cloning for configured retries.
- Passive circuit recovery stores and compares Unix epoch seconds even though it is process-local elapsed time. Replace it with one process-monotonic origin, use saturating boundary arithmetic, and preserve the one-winner CAS and backoff update ordering. Keep HTTP-date `Retry-After` on wall-clock time.
- Exact proposed code allow-list: `outbound/call.rs` plus a documentation-only monotonic-time correction in `outbound/client.rs`; colocated tests remain in `call.rs`. Failure detail, endpoint-per-retry, response-body cap and logical-call deadline semantics stay unchanged.
- The 20 network-backed MockServer cases inside the library unit-test module require a standalone self-contained test-isolation task when this batch promotes; existing integrations remain frozen.
