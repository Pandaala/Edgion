---
name: relist-recompiles-every-generation
description: A conf_sync relist recompiles every matchable EdgionTls's cold state even when content is unchanged. Accepted deliberately. Reject re-reports proposing memoization unless the re-evaluation triggers below are met.
knowledge_type: intentional-design
---

# Relist recompiles every generation — accepted

**Status:** ⚠️ accepted tradeoff · 2026-09-21 (design review of D0014)

## Decision

`TlsStore::replace_all_tls` calls `CompiledEdgionTls::compile` for every
`matchable` entry on every relist, with no check for unchanged content. This is
accepted. Do not add memoization without meeting the re-evaluation triggers.

## Why the cost is acceptable

- **The data plane is not blocked.** Handshakes read the matcher's `ArcSwap`
  (`TlsCertMatcherSnapshot`) and never touch `CacheData`. `replace_all_tls`
  builds the complete next generation and publishes it with a single
  `self.matcher.data.store(...)` as its last statement, so every in-flight and
  arriving handshake keeps serving the previous generation, lock-free, for the
  whole recompile. There is no handshake latency and no connection impact.
- **It is one single-threaded pass** on the conf_sync task, bounded by the
  resource count, not a repeated or concurrent cost.
- **Relist is normal, expected operation** — watch decode failure, event apply
  failure, channel reinstall, reconnect backoff. Paying a bounded CPU burst on a
  normal control-plane event is the intended shape.
- **It is not a new class of work.** Before cold compilation existed, this same
  path already ran `validate_cert` per entry, which performs `cert_pem()` /
  `key_pem()` Secret-to-String clones, `parse_leaf_cert`,
  `check_chain_expiration`, and `check_key_matches_cert` — the last running
  `parse_strict_certificate_chain` plus `parse_strict_private_key_pem`. Cold
  compilation raised this cost; it did not introduce it.
- The Gateway listener matcher's `rebuild_from_gateways`
  (`edgion-gateway/src/runtime/matching/tls.rs`) has the same unconditional
  shape, so this is the established cost model for publication-time compilation,
  not an EdgionTls-specific regression.

## Why memoization is not a free win

Both obvious identity keys were checked against current code and do not work:

- **`ResourceMeta::get_sync_version()` gives zero hits here.**
  `publish_list_payload`
  (`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs`) stamps *the
  list's* sync version onto every resource before `CacheData::reset`, so the
  annotation identifies the relist generation, not per-resource content — every
  resource gets a new value on every relist. Separately,
  `extract_sync_version` returns `0` when the annotation is absent, so on any
  path that does not stamp, two different generations both read `0`; matching on
  that would reuse stale compiled credentials and keep serving a rotated-away
  certificate.
- **`PartialEq` is not available on the resource.** `EdgionTlsSpec` derives
  `CustomResource, Serialize, Deserialize, JsonSchema` only, and the spec tree
  holds runtime-only cache state with no meaningful equality —
  `resolved_listener_attachment_index: OnceLock<HashSet<..>>` and the compiled
  regexes inside `client_auth.allowed_sans`.

A correct memoization would need a fingerprint over the real compilation inputs
(`tls.crt` / `tls.key` / `ca.crt` bytes plus `spec.ciphers`), and that
fingerprint must cover every input the compiled state depends on — otherwise a
rotation touching only an uncovered input is silently ignored. That is a
deliberate design with a rotation-correctness failure mode, not a cleanup.

`Arc::ptr_eq` cannot substitute: `CacheData::reset` rebuilds every resource as a
fresh `Arc`, so pointer identity never holds across a relist. The partial-update
path is unaffected and does share untouched keys' compiled state (see
`partial_update_shares_compiled_state_for_untouched_keys`).

## Re-evaluation triggers

- Measured relist CPU becomes a real operational problem at deployed fleet size.
- A trustworthy per-resource content identity appears on the conf_sync path
  (a content hash carried by the controller, or a resource-level version that is
  stamped per resource rather than per list).
- Compilation grows materially more expensive per entry.
- The handshake path ever gains a dependency that makes publication block
  serving — the "data plane is not blocked" premise above is load-bearing.
