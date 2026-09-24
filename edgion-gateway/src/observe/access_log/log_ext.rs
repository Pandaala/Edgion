//! The `ext` block: operator-declared access-log fields.
//!
//! Holds resolved `(key, value, sensitive)` triples and serializes them as one
//! flat JSON object under `ext`.
//!
//! **The buffer holds text, never hand-written JSON fragments.** `ext` values are
//! header / query / cookie content — attacker-controlled. A hand-formatted
//! `"k":"v"` byte buffer would let a value containing `"` tear the entry open and
//! forge fields (a fake `"status":200`). That is log injection, not a formatting
//! bug. `PluginLog` is the precedent: its buffer holds text and a custom
//! `Serialize` impl lets serde escape. Same reason `push_err` takes
//! `&'static str`.

use edgion_resources::resources::common::log_labels::{LOG_LABEL_MAX_COUNT, LOG_LABEL_MAX_VALUE_LEN};
use edgion_resources::sensitive::apply_sensitive;
use serde::{ser::SerializeMap, Serialize, Serializer};
use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::EdgionHttpContext;
use edgion_resources::resources::edgion_gateway_config::UnmaskedKeys;
use edgion_resources::resources::edgion_plugins::{AccessLogExternField, LogExtSource};
use pingora_proxy::Session;

/// One resolved field.
struct LogExtField {
    key: String,
    value: String,
    /// When true the value renders through `apply_sensitive` (off → clear,
    /// replace → `***`, wrap → `<E>value</E>`).
    sensitive: bool,
}

/// The entry's `ext` block: a small ordered vec, not a map.
///
/// Insertion order across tiers is the serialized order and is a contract for
/// downstream parsers and integration assertions. A linear scan over ≤16 entries
/// beats a `BTreeMap` and avoids its per-request allocations — this runs on the
/// data-plane path.
#[derive(Default)]
pub struct LogExt(Vec<LogExtField>);

impl LogExt {
    /// Insert or overwrite `key`.
    ///
    /// Later tiers overwrite earlier ones **in place**, so the key keeps its
    /// original position. Values are truncated to `LOG_LABEL_MAX_VALUE_LEN` on a
    /// UTF-8 char boundary. At the cap a *new* key is dropped, but an existing
    /// key still updates — otherwise a full map would silently ignore the route
    /// tier's override of a global field.
    pub fn insert(&mut self, key: &str, value: impl Into<String>, sensitive: bool) {
        let existing = self.0.iter().position(|field| field.key == key);
        if existing.is_none() && self.0.len() >= LOG_LABEL_MAX_COUNT {
            return;
        }

        // Dynamic sources already resolve to `String`; accepting ownership
        // avoids copying every projected value again on the request path.
        let mut value = value.into();
        truncate_on_char_boundary(&mut value, LOG_LABEL_MAX_VALUE_LEN);
        if let Some(index) = existing {
            self.0[index].value = value;
            self.0[index].sensitive = sensitive;
            return;
        }
        self.0.push(LogExtField {
            key: key.to_string(),
            value,
            sensitive,
        });
    }

    /// Used by `skip_serializing_if`: an empty block is omitted entirely.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Number of fields currently held.
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// Truncate to at most `max` bytes without splitting a UTF-8 char.
fn truncate_on_char_boundary(value: &mut String, max: usize) {
    if value.len() > max {
        let mut end = max;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }

    // `String::truncate` keeps the original allocation. Resolved values may
    // come from attacker-controlled headers/query parameters, so retaining a
    // large capacity in each of the bounded fields would undermine the value
    // size cap until the access-log entry is serialized.
    if value.capacity() > max {
        *value = value.as_str().to_owned();
    }
}

impl Serialize for LogExt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for f in &self.0 {
            if f.sensitive {
                // `apply_sensitive` is NOT re-entrant in wrap mode: it holds a
                // thread-local borrow for the closure's duration. `serialize_entry`
                // does not call back into it, so this is safe — do not nest.
                apply_sensitive(&f.value, |rendered| map.serialize_entry(&f.key, rendered))?;
            } else {
                map.serialize_entry(&f.key, &f.value)?;
            }
        }
        map.end()
    }
}

/// Whether a resolved field's value must render through `apply_sensitive`.
///
/// Category 1 (`routeLabel` / `routeAnnotation`) is never masked: the values are
/// authored by the operator in the route's own metadata — configuration, not
/// request data. (The *selection* is constrained instead: reserved annotation
/// prefixes are un-nameable — reported on the resource's `Accepted=False`
/// status condition at ingest, and dropped by the gateway when it loads the
/// resource.)
fn should_mask(whitelist: &UnmaskedKeys, source: LogExtSource, name: &str) -> bool {
    match source {
        LogExtSource::RouteLabel | LogExtSource::RouteAnnotation => false,
        _ => !whitelist.allows(source, name),
    }
}

/// Default whitelist, used when the gateway declares no `AccessLogExtern`
/// policy at all. Built once: `UnmaskedKeys::default()` alone is 11 `String`
/// allocations, which the per-request path must not pay.
static DEFAULT_UNMASKED: LazyLock<UnmaskedKeys> = LazyLock::new(UnmaskedKeys::default);

/// Resolve every declared field for this request into the `ext` block.
///
/// Runs at `logging()`, so it sees every request — including the ones that never
/// reached a plugin chain (404, 421, the 400 family, gRPC UNIMPLEMENTED, ACME,
/// CORS preflight) and the ones an earlier Deny terminated (403 / 429).
pub fn collect(session: &Session, ctx: &EdgionHttpContext) -> LogExt {
    let mut ext = LogExt::default();

    // Borrow the store rather than cloning its `Arc` (an atomic RMW pair) — every
    // request reaches this point, and the common case is the fast path just below,
    // which must cost a single lock-free snapshot load and nothing more.
    let Some(owner) = ctx.plugin_scope() else {
        return ext;
    };
    let store = &owner.scope;
    if !store.has_access_log_extern {
        return ext;
    }
    let global_keys = &store.global_keys;
    let gateway_keys = &owner.http_keys;
    let route_keys: &[(String, String)] = match (&ctx.route_unit, &ctx.grpc_route_unit) {
        (Some(u), _) => u.plugin_runtime.ext_ref_keys(),
        (_, Some(u)) => u.plugin_runtime.ext_ref_keys(),
        _ => &[],
    };

    // No ref from this request into any EdgionPlugins at all: nothing to resolve
    // regardless of what the fleet-wide flag above says.
    if global_keys.is_empty() && gateway_keys.is_empty() && route_keys.is_empty() {
        return ext;
    }

    let whitelist = store
        .config
        .as_ref()
        .and_then(|config| config.spec.access_log_extern.as_ref())
        .map(|policy| &policy.unmasked_keys)
        .unwrap_or(&DEFAULT_UNMASKED);

    // Tier 1: declarations from the request's captured GatewayClass scope.
    for key in global_keys {
        apply_declarations(store, key, whitelist, session, ctx, &mut ext);
    }
    // Tier 2: Gateway annotation (edgion.io/edgion-plugins).
    for key in gateway_keys {
        apply_declarations(store, key, whitelist, session, ctx, &mut ext);
    }
    // Tier 3: route ExtensionRef. Last, so a route field shadows the tiers above
    // it (documented risk: name global fields with a distinct prefix).
    for (ns, name) in route_keys {
        let key = format!("{}/{}", ns, name);
        apply_declarations(store, &key, whitelist, session, ctx, &mut ext);
    }

    ext
}

/// Read one `EdgionPlugins`' declarations from the live store and apply them.
/// A dangling ref is silently skipped: this is the data-plane path, where
/// `tracing` is forbidden.
fn apply_declarations(
    store: &crate::plugins::publication::PluginScope,
    key: &str,
    whitelist: &UnmaskedKeys,
    session: &Session,
    ctx: &EdgionHttpContext,
    ext: &mut LogExt,
) {
    if let Some(compiled) = store.http(key) {
        for field in compiled.spec.access_log_extern_fields() {
            if let Some(value) = resolve_field(field, session, ctx) {
                let sensitive = should_mask(whitelist, field.from, &field.name);
                ext.insert(&field.key, value, sensitive);
            }
        }
    }
}

/// Read one field's value from its source. `None` means "the source had no
/// value" — the field is then absent from `ext`, which is distinct from a
/// whitelist miss (present, masked).
fn resolve_field(field: &AccessLogExternField, session: &Session, ctx: &EdgionHttpContext) -> Option<String> {
    // Route metadata, resolved once at route build (Step 3). `MatchInfo` (HTTP)
    // and `GrpcMatchInfo` (gRPC) are distinct types with identical fields, so
    // each side is picked explicitly. Absent on a 404, where neither unit exists.
    let labels_meta = || -> Option<&BTreeMap<String, String>> {
        if let Some(u) = ctx.route_unit.as_ref() {
            return u.matched_info.labels_meta.as_deref();
        }
        ctx.grpc_route_unit
            .as_ref()
            .and_then(|u| u.matched_info.labels_meta.as_deref())
    };
    let annotations_meta = || -> Option<&BTreeMap<String, String>> {
        if let Some(u) = ctx.route_unit.as_ref() {
            return u.matched_info.annotations_meta.as_deref();
        }
        ctx.grpc_route_unit
            .as_ref()
            .and_then(|u| u.matched_info.annotations_meta.as_deref())
    };

    match field.from {
        // Repeated headers: `HeaderMap::get` returns the FIRST value. Caveat to
        // document, not to defend against: if the upstream is last-wins, an
        // attacker can make the log record a value the backend did not act on.
        // The access log records what the gateway saw at entry, not what the
        // backend processed. Non-UTF-8 values yield None (field absent).
        LogExtSource::Header => session
            .req_header()
            .headers
            .get(&field.name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        LogExtSource::RespHeader => session
            .response_written()
            .and_then(|h| h.headers.get(&field.name))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
        // Entry-time raw query, NOT the rewritten URI — same init-once cache the
        // plugins and the `${query:...}` template already share, so the parse is
        // paid at most once per request.
        LogExtSource::Query => ctx
            .cached_query(ctx.request_info.entry_raw_query.as_deref())
            .get(&field.name)
            .cloned(),
        LogExtSource::Cookie => {
            crate::plugins::runtime::session_adapter::cookie_value(&session.req_header().headers, &field.name)
        }
        LogExtSource::Ctx => ctx.get_ctx_var(&field.name).map(|s| s.to_string()),
        LogExtSource::RouteLabel => labels_meta().and_then(|m| m.get(&field.name).cloned()),
        LogExtSource::RouteAnnotation => annotations_meta().and_then(|m| m.get(&field.name).cloned()),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/observe/access_log/log_ext_test.rs"]
mod tests;
