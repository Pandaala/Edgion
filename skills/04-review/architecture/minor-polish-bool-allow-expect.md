---
name: minor-polish-bool-allow-expect
description: Decide on three recurring "polish" findings — weak-type bool parameter when doc is explicit, `#[allow(dead_code)]` without a comment on wire-protocol fields, and `init_crypto` using `.expect` at process entry. All three are intentional or have higher-cost-than-benefit fixes.
---

# Minor Polish — bool / allow(dead_code) / .expect

**Audit date**: 2026-05-16
**Linked issue**: P3-cc-012 (closed; no code change)
**Conclusion**: The three sub-items below are style preferences or design choices, not defects. Re-reporting closes without action.

## Findings Not Accepted

### (a) `compose_admin_routes(.., local_auth_intent: bool)` should be an `enum LocalAuthIntention { Enabled, Disabled }`

**Obsolete (2026-06-12)**: this finding no longer applies. Admin API auth went **token-only** (Istio-style pre-shared bearer); the browser-login stack was removed, taking `compose_admin_routes` and its `local_auth_intent: bool` parameter with it. There is no longer a bool-vs-enum question here. The Admin API onion is now assembled directly in `edgion-controller/src/api/mod.rs::serve_with_shutdown` with no `local_auth_intent` flag. Do not re-file.

### (b) `#[allow(dead_code)] sync_version: u64` on `WatchEventRaw` should carry a `// TODO: …` comment

Location: the Center `fed_sync::server` `WatchEventRaw` struct (Center code is now maintained in the separate EdgionCenter repository).

**Why not accepted**:

1. The struct is `#[derive(serde::Deserialize)]` with `#[serde(default)] sync_version: u64`. The field is part of the wire protocol contract between Controller and Center; the **outer** `resp.sync_version` (lines 408/418/558/567) is what the Center actually applies, but the inner per-event `sync_version` must still parse to remain forward-compatible with future Controller versions that may populate it.
2. The `#[allow(dead_code)]` is on a `serde`-deserialized field, not on a dead function or unused local — its purpose is structural (match the wire format) rather than placeholder code. Adding `// TODO: use when …` would invite future contributors to "use it" and accidentally couple Center logic to a field the Controller doesn't authoritatively own.
3. Whatever explanation the comment carries (e.g., "per-event sync_version is wire-only; outer resp.sync_version is canonical") is already inferable from reading the 30 lines of context above — the struct-level doc on `WatchEventRaw` (lines 38-42) does the substantive job.

**Exception**: if a future change starts using the inner `sync_version` (e.g., per-event watermarking), drop the `#[allow]` entirely rather than commenting it.

### (c) `init_crypto()` should use `.ok()` instead of `.expect()` for test-harness friendliness

Location: `edgion-common/src/startup.rs:3-7`.

**Why not accepted**:

1. The three callers are the top-level `main()` functions of
   `edgion_controller`, `edgion_gateway`, and `edgion_ctl`. The Center binary is
   maintained in the separate EdgionCenter repository. Each in-repo binary is a
   single-process entry; **no Rust test or library code calls `init_crypto`**.
   The "twice-called in a test harness" scenario described in the finding does
   not exist — integration tests launch one binary per process.
2. At a binary entry point, `.expect` is the correct policy: if `install_default()` returns `Err`, it means some other code already installed a different `CryptoProvider`. Silently `.ok()`-ing would let the binary continue with an unknown provider while we believed we installed `ring` — a real-world security configuration drift becomes invisible.
3. If a Rust integration test ever needs to install crypto in-process, the right shape is `let _ = rustls::crypto::ring::default_provider().install_default();` inside the test (or a `OnceLock`), **not** a permanent downgrade of the production main-entry helper.

**Exception**: when a Rust-level test actually needs to share a process with crypto-using code, add a separate `init_crypto_for_tests()` helper that returns `Result` or uses `OnceLock`; leave `init_crypto()` strict for production binaries.

## Cross-link

These three rejected fixes were filed together as P3-cc-012 ("Naming, doc, and minor polish"). If a future review re-files any of them under a different ID, close per the rationale above without code change.
