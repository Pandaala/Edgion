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

### (e) `runtime/store/config.rs` (formerly 622 lines, deleted in runtime-F003) "modify `is_ready` against `ConfigClient::required_cache_names`"

- **Cites a non-existent API**: `required_cache_names` did not appear in repo grep; the file also had no `is_ready` function; its single responsibility was `GatewayListenerConfig` query logic. The whole file has since been removed as orphan code (runtime-F003), which retroactively confirms the original finding's premise as wrong on yet another axis.
- **Verdict**: finding's premise is wrong; close.

### (f) `src/core/common/unified_auth/mod.rs` (808 lines = 559 non-test + 249 test) "should split into state/middleware/oidc/local"

- **Story line is complete**: types → state construction → middleware main flow → two validators → helpers; linearly readable, no duplication, no coupling leak.
- **Splitting is negative-value**: `try_oidc_validation` / `try_local_validation` are thin adapters over `AuthMiddlewareState` / `LocalAuthState` and are not externally referenced; after splitting, readers must jump among 3 files to understand one request.
- **The only marginally doable small cleanup**: `UnifiedAuthState::from_configs` (~135 lines, startup-time construction) could be moved into `state.rs` separately, with the rationale of "startup vs hot path" separation — but the benefit is weak and does not constitute P2/P3.
- **Verdict**: split not accepted. 559 non-test lines is not large within this project (compare `edgion_acme.rs` 1491 lines, `config.rs` 622 lines).

### (g) `src/core/gateway/api/mod.rs` (1644 lines) "should split into admin / testing / link_sys submodules + dedup 3 `get_*_client_by_name` helpers + merge `serve` / `serve_with_shutdown`" (infra-cluster-F003, 2026-05-27)

- **Fails the duplication evidence bar**: the three `get_redis_client_by_name` / `get_etcd_client_by_name` / `get_es_client_by_name` helpers are ~15 lines each (≈45 lines total), below the ≥30-line-per-near-duplicate threshold this skill requires. The bodies also pin a per-provider concrete client type (`RedisLinkClient` / `EtcdLinkClient` / `EsLinkClient`) — a generic helper would need a trait + per-provider impl, expanding surface without reducing reader load.
- **No coupling leak**: the only `pub` symbol with an external caller is `serve_with_shutdown` (single site at `src/core/gateway/cli/mod.rs:103`); `pub fn create_admin_router` has zero external grep callers (only the in-file `serve` and `serve_with_shutdown` consume it); testing handlers and `get_*_client_by_name` helpers are not referenced from outside the file. Splitting changes nothing about the public surface.
- **Story line is linear**: admin core router build → conditional testing-mode merge → testing handlers (access-log + store-stats + LinkSys) → bind & serve. A single startup story currently reads top-to-bottom; splitting forces readers to jump across `api/admin.rs` / `api/testing/mod.rs` / `api/testing/link_sys/*.rs` to follow one boot path.
- **Severity Low + zero runtime impact**: the finding itself acknowledges "no runtime impact". Refactoring 1600+ lines for maintainability-only gain is exactly the kind of churn this skill exists to reject.
- **The `serve` / `serve_with_shutdown` sub-point is already covered by F-014** (its own narrow scope: delete dead `serve()` or implement it as a thin wrapper, per the controller's pattern at `controller/api/mod.rs:457` — `pub async fn serve` body is a single delegation to `serve_with_shutdown(..., std::future::pending::<()>())`). F-003 should not bundle that work — F-014 is the correct unit.
- **Verdict**: ⚠️ accepted tradeoff. Re-open only on (i) a third `get_*_client_by_name` clone landing for a fourth provider (then dedup the helper alone, single-file change — not a whole-file split); (ii) external code starting to import handler symbols from this file (real coupling leak); (iii) a concrete operability incident traced to the file size (e.g. a bug fix that demonstrably required cross-referencing two unrelated handlers).

## When to Re-Evaluate

- Any of the above files develop **real** line-level duplication (≥ 2 places with 30+ similar blocks); re-evaluate (d).
- `unified_auth` adds ≥ 1 new provider (e.g., SAML); the split value of (f) rises significantly.
- A grep-verifiable "stable public API commitment" mechanism is added; re-evaluate (b).

## Fix Suggestions Not Accepted (list)

- "File X has too many lines, should be split" — close directly without the three pieces of evidence.
- "Two modules look like they both do Y; should extract common code" — close directly without ≥ 2 places of 30+ line-level near-duplicate evidence.
- "Adding X automatically becomes public API" — close directly without distinguishing binary / lib projects.
- "Single-file admin/testing router mixes admin core + integration-test routes + LinkSys testing handlers + serve helpers, should be split into submodules" — close per (g); pure-position-only refactor of a 1600-line file with no coupling leak and no real duplication. The narrow `serve` / `serve_with_shutdown` dedup is a separate issue (infra-cluster F-014), not a reason to bundle a whole-file split.
