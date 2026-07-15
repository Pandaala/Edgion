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

Typical example: `fn default_true() -> bool { true }`, `fn is_true(v: &bool) -> bool { *v }` scattered in 13+ places under `edgion-resources/src/resources/**`.

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

**Accepted as a real finding (verdict updated 2026-05-29).** The project rule is now
unambiguous: **all content written into source files must be English** — identifiers,
`//` / `///` / `/* */` comments, doc comments, TODO/FIXME, and log/error literals
(see both the global and project `CLAUDE.md`). The `python3 cicd/checks/validate_agent_docs.py` /
`check-english-only` lint enforces this. The only on-disk Chinese exception is
user-facing documentation under `docs/zh-CN/`.

So a Chinese internal comment in a `.rs` file is a defect, not a "convention" — translate
it to English. The earlier "internal explanations may be Chinese / translating loses
precision" stance recorded here (audited 2026-04-26, before the English-only rule
hardened) is superseded; do **not** decline such findings on that basis.

**Still true**: switching languages mid-sentence is obviously a defect; and a pure
*style* unification of two already-English comments (e.g. "make all these say `// returns
N`") with no semantic drift is the low-value churn this file is about — that part is still
not, by itself, a finding.

## Already-Performed Cleanup (to avoid re-filing)

- External task numbers like `tasks/fix-change/I*/M*/R*/C1/D2/S3` scattered in comments (linked tasks no longer exist) have been removed in the following files, while keeping substantive semantics:
  - `edgion-common/src/link_sys_core/credential.rs`
  - `edgion-resources/src/resources/common/tls.rs`
  - `edgion-resources/src/credentials/materials.rs`
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs`
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`
  - `edgion-controller/src/conf_mgr/conf_center/etcd/config.rs`

If the strings `fix-change` or orphan codes like `I4/M6/C1` are spotted again in the future — those are newly introduced; treat as dead links; no need to scan the 6 already-cleaned files again.
