---
name: acme-service-shape
description: Decide on ACME `service.rs` "huge phase function should be split / global singleton should use DI" findings. Audited as design tradeoff; refactoring fix suggestions not accepted.
---

# ACME service.rs Shape — Audited Tradeoff

**Audit date**: 2026-04-26
**Linked issue**: P1-05 (closed, only the code-defect parts fixed; no architectural refactoring)
**File**: `src/core/controller/services/acme/service.rs` (~1158 lines)

## Already Fixed (former real defects)

- Status write-back was silently dropped → now `if let Err(status_err) = update_acme_status(...)` logs `tracing::warn!` (service.rs:395-409)
- Non-UTF-8 DNS credentials silently fell back to base64 → now explicit `String::from_utf8(...).map_err(...)?` returns Err (service.rs:751)

Only newly emerging cases of these two categories (swallowed error / non-UTF-8 silent fallback) are accepted as findings.

## Fix Suggestions Not Accepted (after this audit)

### 1. "`execute_http01_flow` / `execute_dns01_flow` are 90 lines too long; should split into phase sub-functions"

- ACME protocol is a naturally linear flow: account → order → challenge → finalize → certificate; each step depends on the previous.
- The split sub-functions are each called once by this flow only, with long parameter lists (client, acme, status, challenge state…); readers would jump back and forth among 5–6 files.
- "Linear story sliced horizontally" is negative-value, hitting a failed acceptance against the triple in `architecture/size-and-similarity-heuristics.md`.
- **Verdict**: split not accepted.

### 2. "`handle_resource_check` (~173 lines) should be split"

- The bulk is the inline block inside `tokio::spawn` (success/failure/retry-scheduling three-branch).
- Extracting to `async fn run_acme_flow(...)` is a mechanical operation; behavior unchanged; benefit is only "one less indentation level".
- Optional; does not constitute P1/P2 finding; filing such suggestions requires "real reader confusion" evidence.

### 3. "`ACME_SERVICE: Mutex<Option<AcmeServiceHandle>>` global singleton should switch to `Arc<...>` injected into HandlerContext"

- The Controller is a leader-elected single-active model; multi-instance concurrency is not a design goal.
- The only pain is unit-test inconvenience, but the ACME flow bottleneck is K8s API + external ACME server; switching instantiation method still cannot test it; it just moves the mutex.
- The change cascades to HandlerContext + all handler signatures; benefit is illusory.
- **Verdict**: DI refactoring not accepted. If in the future ACME really needs unit tests or multi-tenant splitting, file a new targeted issue.

## When to Re-Evaluate

- ACME service really needs multi-instance support (e.g., multi-tenant isolation) → re-evaluate #3.
- A real unit-testable logic separation appears (e.g., extracting the ACME state machine into a pure-function module) → re-evaluate the split value of #1, #2.
- New swallowed errors / silent fallbacks appear (status write-back, external IO, key parsing) → file as a normal finding under "Already Fixed".
