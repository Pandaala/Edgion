# conf-sync F-011: ConfigClient typed-accessor / 17-arm block deduplication

- **Severity**: Low
- **Dimension**: Redundancy (maintenance surface)
- **Status**: ✅ RESOLVED (2026-06-07) — deduplicated. Safe to delete this file.

## What was actually found (corrections to the 2026-05-28 snapshot)

By the time this was revisited, three of the five parallel structures had already
been collapsed onto `get_dyn_cache()`:

- `DynClientCache` already carried `kind_name_dyn / len_dyn / keys_dyn`.
- `client_cache_stats()` and `client_cache_keys()` already iterated
  `ALL_RESOURCE_INFOS` and dispatched through `get_dyn_cache()`.

Two assumptions in the original finding were **wrong** and were verified by grep:

1. The typed accessor block was claimed to have a real `&ClientCache<X>` consumer
   (`lb/lb_policy/policy_store.rs`). It does not — `policy_store.rs` only calls
   `trigger_endpoint_slice_update_event(...)`. The 17 accessors were used **only**
   inside the two `api/mod.rs` macros, each immediately taking `.list().data`.
2. There was an extra, undocumented parallel 17-arm block: the generic
   `list(&self, _key, kind) -> Result<ListDataSimple, String>` method. It had
   **zero callers** and serialized `EdgionTls` **without** private-key scrubbing
   (a latent leak had it ever been wired up).

## What was done

1. `DynClientCache` gained `list_json_dyn()` (type-erased JSON listing, no redaction).
2. `api/mod.rs`: the two 17-arm macros (`list_client_resources!`,
   `get_client_resource!`) became plain functions that dispatch generically via
   `get_dyn_cache(kind)`. The namespaced-vs-cluster distinction now comes from
   `is_kind_cluster_scoped(kind)`; the get path reuses the list path and filters
   by `metadata.name` / `metadata.namespace`.
3. `EdgionTls` is the **only** special case: it still goes through the typed path
   (`to_json_vec_edgion_tls` → `scrub_edgion_tls_secrets`) so private-key scrubbing
   stays type-safe. Its accessor `edgion_tls()` is the one retained accessor.
4. Deleted: the other 16 typed accessors; the dead generic `list()` method and the
   gateway-side `ListDataSimple` struct + its two re-exports; the now-unused
   `find_resource` / `find_region_resource` / `find_edgion_tls` / `to_json_vec`
   helpers in `api/mod.rs`.
5. Kept: the `list_xxx()` typed methods (real typed callers in
   `runtime/server/base.rs`, `cli/pingora.rs`, `backends/preload.rs`; not a
   ResourceKind match, so no exhaustiveness risk).

Verification: `cargo fmt --check`, `cargo check --all-targets`, `cargo clippy`
(no new warnings in touched files), `make check-agent-docs`, and the
`admin_serialization_strips_private_keys` test (now exercises the get path through
a real `ConfigClient`) + `config_client` / `cache_client` / `store_stats` tests
all pass.

## Out of scope (noted, not addressed)

`EdgionPlugins` (`header_cert_auth.rs: resolved_ca_secrets`) and `Gateway`
(`resolved_frontend_ca_secrets`) carry resolved secret material that the admin API
does **not** currently scrub. Pre-existing behavior, unchanged here. If those should
be redacted on the admin API, that is a separate security task — and a type-owned
redaction hook (the "Approach 2" considered during brainstorming) would be the
natural way to cover all secret-bearing kinds uniformly.
