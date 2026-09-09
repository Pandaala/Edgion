# B0245 — HTTP Common Outbound Call

## Scope

- Pipeline state: changed; implementation, independent reviews and verification complete, awaiting local commit.
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

## Promotion

- Current HEAD: `654fe9a1`. Target and supporting client blobs remain exactly `de92374aa0d00e84c4cb4966734427f7249e6095` and `b26c50682f43165dcb74af512c56e4661335ea48`; B0244 did not touch this call chain.
- Frozen code allow-list: `edgion-gateway/src/plugins/http/common/outbound/call.rs` and a documentation-only timing correction in `edgion-gateway/src/plugins/http/common/outbound/client.rs`.
- Coordinator allow-list also includes the self-contained `tasks/todo/isolate-shared-outbound-call-unit-tests-from-network.md` task and campaign records. No existing integration file may change.

## Outcome

- The default sole-attempt path now transfers the already-built `reqwest::Request` directly instead of cloning its URL, headers and metadata. Retry-enabled calls retain per-attempt `try_clone` and the exact existing non-cloneable error and retry state machine.
- Passive circuit recovery now uses process-monotonic elapsed seconds from one `OnceLock<Instant>` origin. Eligibility preserves disabled-backoff behavior with `interval == 0`, uses saturating nonzero boundaries, and retains the single non-looping CAS plus winner-only backoff update. HTTP-date `Retry-After` remains wall-clock based.
- Corrected the passive recovery gate documentation and removed stale references to a deleted Webhook health module. Added four deterministic no-network tests for request ownership/replayability and elapsed-time boundaries.
- Independent plan reviewer `b0245_plan_review` required and approved the explicit zero-interval branch and reversed-sample regression. Independent final reviewer `b0245_final_review` required three documentation/test corrections, then returned LGTM with no remaining findings.
- Verification: outbound call 34/34, ForwardAuth 6/6, AI Guard 49/49, Guardrail 20/20 and Webhook 46/46. Workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed.
- Deferred: D0128 tracks isolation of the remaining network-backed library unit tests without editing existing integrations.
- Commit: pending; never push.
