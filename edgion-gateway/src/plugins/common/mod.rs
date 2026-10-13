//! Cross-protocol shared plugin building blocks.

pub(crate) mod geo_ip_refs;
pub mod ip_acl;
pub mod redis_rate_limiter;

/// Identity of one plugin entry: `<resource key>/<alias or #index>`, i.e.
/// `<namespace>/<name>/<alias or #index>` for a namespaced resource.
///
/// Plugins that keep state outside their own instance — Redis buckets, a
/// process-global registry — must namespace it by the entry that owns it.
/// The compiled runtime is built once per resource (`EdgionPlugins` /
/// `EdgionStreamPlugins`) and Arc-shared by every route referencing it, so this
/// string names exactly the sharing unit: no more (two resources stay separate
/// even with an identical config) and no less (every Gateway instance derives
/// the same string from conf_sync, so cluster-wide counters still add up, and it
/// is stable across hot reload).
///
/// `resource_key` must be the store key (`ResourceMeta::key_name`), so two
/// scopes can only collide when the two resources are the same store entry.
///
/// `alias` — HTTP entries only; stream entries have no alias field — is already
/// validated against `^[a-zA-Z0-9._-]{1,32}$`, and a duplicate within a stage is
/// reported and dropped before it reaches here, so an alias can neither inject a
/// separator nor be claimed twice; the `#` on the index form keeps entry 1 from
/// colliding with an entry aliased `1`.
///
/// The scope carries no stage component: `requestPlugins[0]` and
/// `upstreamResponsePlugins[0]` both render to `<resource key>/#0`. That is
/// sound only because the request stage is the sole consumer today (aliases are
/// deduplicated per stage array, not per resource). A response-stage plugin that
/// starts keeping external state must add a stage segment first.
pub fn entry_scope(resource_key: &str, alias: Option<&str>, index: usize) -> String {
    match alias {
        Some(alias) => format!("{}/{}", resource_key, alias),
        None => format!("{}/#{}", resource_key, index),
    }
}
