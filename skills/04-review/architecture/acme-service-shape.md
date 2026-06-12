---
name: acme-service-shape
description: Decide on ACME `service.rs` "huge phase function should be split / global singleton should use DI" findings. Audited as design tradeoff; refactoring fix suggestions not accepted.
---

# ACME service.rs Shape — Audited Tradeoff

**Audit date**: 2026-04-26
**Linked issue**: P1-05 (closed, only the code-defect parts fixed; no architectural refactoring)
**File**: `edgion-controller/src/services/acme/service.rs` (~1158 lines)

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

### 4. "`update_acme_status` / `patch_active_challenges` use `PatchParams::apply(...).force()` unconditionally; should drop `.force()` or implement try-then-force-on-409"

- `EdgionAcme` is an Edgion-owned CRD; the realistic field-manager population is exactly two — `edgion-acme-controller` (status fields) and `edgion-acme-challenges` (active challenges only).
- The two managers do not overlap on any field: `EdgionAcmeStatus.conditions` and `active_challenges` both carry `#[serde(skip_serializing_if = "Vec::is_empty")]`, so `update_acme_status`'s `Default`-built status omits `active_challenges` from the patch entirely. SSA only manages fields explicitly present in the patch, so the `edgion-acme-controller` writer never touches `activeChallenges` in practice.
- The Controller is a leader-elected single-active model — a second instance of the same manager is structurally impossible.
- The unconditional `.force()` is the project-wide convention for SSA writes against Edgion-owned resources (see `conf_center/kubernetes/storage.rs:325` for the same pattern with `edgion-controller`); changing it only here would diverge from that convention.
- "Drop `.force()`" introduces an unhandled 409 path on the critical status write-back route. "Try-then-force-on-409" introduces a retry/state-machine layer to all 5 call sites for a hypothetical future risk that requires a third-party field manager to ever land on `EdgionAcme/status`.
- **Verdict**: SSA `.force()` fix suggestions on `update_acme_status` / `patch_active_challenges` are not accepted. Re-evaluate only if a third-party controller is actually introduced that writes the same status fields.

## When to Re-Evaluate

- ACME service really needs multi-instance support (e.g., multi-tenant isolation) → re-evaluate #3.
- A real unit-testable logic separation appears (e.g., extracting the ACME state machine into a pure-function module) → re-evaluate the split value of #1, #2.
- New swallowed errors / silent fallbacks appear (status write-back, external IO, key parsing) → file as a normal finding under "Already Fixed".
