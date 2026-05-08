---
name: style-consolidation-thresholds
description: Decide on "consolidate scattered small helpers / different styles into one place" findings. Three-line trivial-function duplication is not an issue; differing-semantics validate_* styles are intentionally distinct.
---

# Trivial Helper Extraction / Style Unification — Acceptance Threshold

**Audit date**: 2026-04-26
**Linked issue**: P3-04 (closed; only dead-link comment cleanup + fed_sync module comment done; the rest not accepted)
**Conclusion**: Duplication itself is not a problem. "Looks duplicated" or "style inconsistent" must be supported by **real maintenance cost** or **reading cost** evidence, otherwise close.

## Findings Not Accepted

### (a) "Three-line trivial helper duplicated across multiple files; should extract common"

Typical example: `fn default_true() -> bool { true }`, `fn is_true(v: &bool) -> bool { *v }` scattered in 13+ places under `src/types/resources/**`.

**Why not accepted**:

1. The function itself has 0 maintenance cost (will never change).
2. After extraction to `resources::common::serde_defaults`, every call site needs `use ...::default_true;` — total line count actually increases.
3. The system prompt is explicit: "Three similar lines is better than a premature abstraction".

**Exception**: if the helper exhibits behavior divergence (one place is `true`, another is `false`, semantically drifted from what should be identical), or the helper itself is complex (≥10 lines + may evolve) — then extraction is for **drift prevention**, not deduplication.

### (b) "`Option<&str>` single-error vs `Vec<String>` aggregate-error two validate styles should be unified"

`WebhookServiceConfig::get_validation_error -> Option<&str>` vs `LinkSys::validate_config -> Vec<String>`.

**Why not accepted**:

1. The semantics are inherently different: the inner structure (webhook config) only cares about "am I valid" → first-error fail-fast; the outer resource (LinkSys) needs to aggregate errors from all variants for the condition message → `Vec<String>`.
2. The current design exactly does `push` of `get_validation_error()` result into `Vec` — the two roles divide responsibilities clearly.
3. Forced unification can only converge to `Vec<String>`, but the inner always produces 0 or 1 element — equivalent to writing one extra line of `vec![]` / `vec![s]`, with zero information gain.

**Exception**: when the inner validate genuinely produces multiple independent errors (one per field), change to return `Vec<String>`; currently webhook only does 6 if-return-Some sites, naturally first-error.

### (c) "Mixed Chinese/English comments should be unified"

Project convention (drawn from actual reading of `secret_ref.rs` / `link_sys/common.rs` and other places): **public-API doc tends to be English, while module/internal explanations may be Chinese**. The "unify to one" suggestion in the audit template ignores this convention — translating internal explanations to English actually loses precision.

**Exception**: switching languages within the same comment paragraph (in the same sentence) is a defect; using different languages across different paragraphs is convention.

## Already-Performed Cleanup (to avoid re-filing)

- External task numbers like `tasks/fix-change/I*/M*/R*/C1/D2/S3` scattered in comments (linked tasks no longer exist) have been removed in the following files, while keeping substantive semantics:
  - `src/core/gateway/link_sys/credential.rs`
  - `src/types/resources/common/tls.rs`
  - `src/types/credentials/materials.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`
  - `src/core/controller/conf_mgr/conf_center/etcd/config.rs`
- `src/core/common/fed_sync/mod.rs` adds a module-level comment explaining that it contains only proto types; runtime logic lives in `controller::fed_sync` / `center::fed_sync` respectively.

If the strings `fix-change` or orphan codes like `I4/M6/C1` are spotted again in the future — those are newly introduced; treat as dead links; no need to scan the 6 already-cleaned files again.
