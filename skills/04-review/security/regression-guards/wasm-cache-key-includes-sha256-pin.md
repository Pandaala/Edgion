---
name: wasm-cache-key-includes-sha256-pin
description: Use when reviewing Wasm module-cache findings that claim the per-resource sha256 integrity pin is skipped on a cache hit; covers why sha256 is folded into the runtime cache key.
status: fixed
finding_id: contract-04
closed: 2026-07-18
knowledge_type: regression-guards
---

# Wasm source-keyed module cache bypasses per-resource SHA verification — FIXED

## Conclusion

The Wasm module cache key (`runtime_cache_key`, `edgion-gateway/src/plugins/http/wasm/vm_manager.rs`)
now includes the normalized `WasmConfig.sha256`. SHA-256 verification lives only inside
`load_inner` (before compilation), and `get_or_load` short-circuits it on a cache hit; before
the fix the key was source-only (`file`/`url`/`oci`/`inline` locator + file stat + runtime-config
digest) and **excluded** `sha256`, so a second plugin pinning a different digest for the same
source received the first plugin's already-verified bytes with no mismatch. Keying on the digest
lands contradictory pins on separate entries, each running its own verification; identical digests
still share one entry (case-insensitive), so there is no efficiency regression.

Re-reports of the form "source-only cache key skips the sha256 pin on a warm entry / plugin B with
digest B gets plugin A's cached bytes / a digest-only reload keeps the old module" are **not
accepted** — verify the `;sha=` term in `runtime_cache_key` first.

## Core Rationale

**1. The check and the cache lived at different layers**

`load_inner` (`:590-595`) computes `Sha256::digest(&bytes)` and rejects a mismatch, but only on the
cold load path. `get_or_load` returns `LoadState::Loaded` before ever calling `load_inner`, so any
config whose key already resolved to a warm entry never re-verified. Folding the pin into the key
is the origin-level fix: a differing pin no longer resolves to the same entry.

**2. Identical pins must still coalesce**

The digest is normalized to lowercase (matching `load_inner`'s `eq_ignore_ascii_case`) and the
absent pin is a distinct empty term, so two plugins that agree on the digest share one compiled
module (the whole point of the cache) while two that disagree do not.

**3. Fetch/TLS config is deliberately still excluded**

`WasmFetchSpec` (credentials, TLS, timeout, retry) does not affect module identity — already
asserted by `fetch_does_not_change_cache_key`. Changing a credential does not require re-fetching
already-verified identical bytes, so it stays out of the key. This finding is orthogonal to
contract-01 (runtime context fields stored on a source-keyed `ModuleEntry`).

## Fix Suggestions Not Accepted

- "Re-hash the cached entry's actual bytes against every caller before returning it" — correct but
  strictly more work than keying on the pin; the key approach verifies once per distinct pin and
  reuses the compiled module, and was the finding's primary suggestion.
- "Add the fetch credential/TLS config to the cache key too" — would force needless re-fetch +
  recompile on a credential rotation for byte-identical, already-verified modules.

## Re-evaluation Triggers

Re-open only if:

- The SHA verification is ever moved out of `load_inner` onto a per-lease path (then the key no
  longer needs the pin, but the reasoning must be re-checked).
- A content-addressed compiled-module store is introduced (the cleaner design the finding mentions),
  which would make the digest the natural primary key and this term redundant.

## Reference Cases

- `tasks/review-wasm-issues/contract-04-source-cache-bypasses-sha-and-fetch-policy.md` (2026-07-18)
- `edgion-gateway/src/plugins/http/wasm/vm_manager.rs` — `runtime_cache_key` (`;sha=` term),
  `load_inner` sha256 verification
- Tests: `runtime_cache_key_separates_by_sha256_pin`, `sha256_pin_enforced_on_warm_cache_hit`
- Sibling cache-key rule: [iron-rules.md](../mandatory-design/iron-rules.md) ("Password / key cache keys")

## Resolution references

- `contract-04` — 2026-07-18.
