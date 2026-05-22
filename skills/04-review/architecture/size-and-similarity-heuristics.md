---
name: size-and-similarity-heuristics
description: Decide on "split / extract common code based on line count or surface similarity" findings. Three preconditions must be verified, otherwise close directly.
---

# Refactoring Suggestions Driven by Line Count / Surface Similarity — Acceptance Triple

**Audit date**: 2026-04-26
**Linked issue**: P2-11 (closed, not fixed)
**Conclusion**: **"Big file / looks similar" alone is not a finding. Three pieces of evidence must be supplied with the finding; otherwise close directly.**

## Three Required Pieces of Evidence

When filing a "should split / should extract common" finding, the finding **must** explicitly include:

1. **Real duplication**: not "both do something", but **line-level near-duplicate** (the same dozens of lines of code blocks appear ≥ 2 times). "Both plugins write 429 responses" is not duplication; "both plugins use the same 30 lines to construct the same ResponseHeader" is.
2. **Real coupling leak**: internal details of the to-be-split target are **already referenced externally** (≥ 2 cross-module direct imports of internal types/constants are grep-able), or they have already split at the type level (e.g., enum variant carrying completely different fields).
3. **Tracking cost decreases after splitting**: when readers follow a single story line (one request / one reconcile / one startup), they **do not need to jump back and forth among the newly-split files**. A linear story sliced horizontally is negative-value.

If any of the three is missing, it is a "based on line count / looks similar"-driven form-only refactor; not accepted.

## Already-Audited Specific Points (covered in this scan; not to be filed again)

### (a) `src/types/resources/common/mod.rs` (124 lines) "route vs LinkSys mixed"

- **Premise wrong**: the `secret_object_ref` / `secret_slot` / `tls` submodules are **shared by all resource types** including `gateway.rs`, `edgion_acme`, `EdgionTls`, `EdgionPlugins`, `link_sys`; not LinkSys-only.
- **Verdict**: false positive. 124 lines is also far below any threshold worth splitting.

### (b) `src/types/mod.rs:53` `pub use self::resources::*` "new resources automatically become public API"

- **Premise does not apply**: Edgion is a binary project (Controller / Gateway / edgion-cli / Center); does not publish a lib; there is no concept of "stable public API boundary".
- **Verdict**: not applicable. Glob re-export inside a binary is a reasonable choice.

### (c) `edgion_acme.rs` parse "all-branch inline big match for ResolveOutcome"

- **Already done**: `outcome_to_resolved_refs_action(outcome, role, …)` exists (around line 322 of the file); `AcmeResolveCtx` / `AcmeResolveAction` enum statically constrains Account vs Dns semantics. Inside `parse()` there is only a thin dispatch + `update_condition`, exactly the shape that the original finding wanted.
- **Verdict**: already fixed; the issue was missed-close.

### (d) `rate_limit` vs `rate_limit_redis` "common logic not extracted"

- **Fails acceptance**: the two operate on different types in `add_headers` / `reject_request` (`isize` vs `i64`, `Rate` local CMS vs Redis Lua script result); the extractable common part is only ~30 lines of header writing and 429 body construction, and would forcibly couple two currently **completely independent** plugins. The `get_header_names()` helper already covers the genuinely reusable part.
- **Verdict**: plugin independence takes priority; "extract common" suggestion not accepted.

### (e) `src/core/gateway/runtime/store/config.rs` (622 lines) "modify `is_ready` against `ConfigClient::required_cache_names`"

- **Cites a non-existent API**: `required_cache_names` does not appear in repo grep; this file also has no `is_ready` function; its single responsibility is `GatewayListenerConfig` query logic.
- **Verdict**: finding's premise is wrong; close.

### (f) `src/core/common/unified_auth/mod.rs` (808 lines = 559 non-test + 249 test) "should split into state/middleware/oidc/local"

- **Story line is complete**: types → state construction → middleware main flow → two validators → helpers; linearly readable, no duplication, no coupling leak.
- **Splitting is negative-value**: `try_oidc_validation` / `try_local_validation` are thin adapters over `AuthMiddlewareState` / `LocalAuthState` and are not externally referenced; after splitting, readers must jump among 3 files to understand one request.
- **The only marginally doable small cleanup**: `UnifiedAuthState::from_configs` (~135 lines, startup-time construction) could be moved into `state.rs` separately, with the rationale of "startup vs hot path" separation — but the benefit is weak and does not constitute P2/P3.
- **Verdict**: split not accepted. 559 non-test lines is not large within this project (compare `edgion_acme.rs` 1491 lines, `config.rs` 622 lines).

## When to Re-Evaluate

- Any of the above files develop **real** line-level duplication (≥ 2 places with 30+ similar blocks); re-evaluate (d).
- `unified_auth` adds ≥ 1 new provider (e.g., SAML); the split value of (f) rises significantly.
- A grep-verifiable "stable public API commitment" mechanism is added; re-evaluate (b).

## Fix Suggestions Not Accepted (list)

- "File X has too many lines, should be split" — close directly without the three pieces of evidence.
- "Two modules look like they both do Y; should extract common code" — close directly without ≥ 2 places of 30+ line-level near-duplicate evidence.
- "Adding X automatically becomes public API" — close directly without distinguishing binary / lib projects.
