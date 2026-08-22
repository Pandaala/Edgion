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

use crate::routes::http::proxy_http::EdgionHttpProxy;
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
    pub fn insert(&mut self, key: &str, value: &str, sensitive: bool) {
        let value = truncate_on_char_boundary(value, LOG_LABEL_MAX_VALUE_LEN);
        if let Some(existing) = self.0.iter_mut().find(|f| f.key == key) {
            existing.value = value.to_string();
            existing.sensitive = sensitive;
            return;
        }
        if self.0.len() >= LOG_LABEL_MAX_COUNT {
            return;
        }
        self.0.push(LogExtField {
            key: key.to_string(),
            value: value.to_string(),
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
fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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
pub fn collect(edgion_http: &EdgionHttpProxy, session: &Session, ctx: &EdgionHttpContext) -> LogExt {
    let mut ext = LogExt::default();

    if ctx.mcp_log_redaction_required() {
        return ext;
    }

    // Borrow the store rather than cloning its `Arc` (an atomic RMW pair) — every
    // request reaches this point, and the common case is the fast path just below,
    // which must cost a single relaxed atomic load and nothing more.
    let store = crate::plugins::http::global_plugin_store();

    // Fast path: no stored EdgionPlugins declares a non-empty `accessLogExtern`
    // anywhere in the fleet. This is the common case — a deployment using
    // rate-limit / CORS / auth plugins but no AccessLogExtern still reaches this
    // point on every request, so the check must be a single relaxed atomic load,
    // not a per-ref plugin-store lookup.
    if !store.has_access_log_extern() {
        return ext;
    }

    let global_refs = edgion_http.edgion_gateway_config.spec.global_plugins_ref.as_deref();
    let gateway_keys = ctx.gateway_info.plugin_store_keys.as_slice();
    let route_keys: &[(String, String)] = match (&ctx.route_unit, &ctx.grpc_route_unit) {
        (Some(u), _) => u.plugin_runtime.ext_ref_keys(),
        (_, Some(u)) => u.plugin_runtime.ext_ref_keys(),
        _ => &[],
    };

    // No ref from this request into any EdgionPlugins at all: nothing to resolve
    // regardless of what the fleet-wide flag above says.
    if global_refs.map_or(true, |r| r.is_empty()) && gateway_keys.is_empty() && route_keys.is_empty() {
        return ext;
    }

    let whitelist = edgion_http
        .edgion_gateway_config
        .spec
        .access_log_extern
        .as_ref()
        .map(|p| &p.unmasked_keys)
        .unwrap_or(&DEFAULT_UNMASKED);

    // Tier 1: GatewayClass-global (EdgionGatewayConfig.spec.globalPluginsRef).
    if let Some(refs) = global_refs {
        for r in refs {
            let ns = r.namespace.as_deref().unwrap_or("default");
            let key = format!("{}/{}", ns, r.name);
            apply_declarations(store, &key, whitelist, session, ctx, &mut ext);
        }
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
    store: &crate::plugins::http::PluginStore,
    key: &str,
    whitelist: &UnmaskedKeys,
    session: &Session,
    ctx: &EdgionHttpContext,
    ext: &mut LogExt,
) {
    store.with_plugin(key, |compiled| {
        for field in compiled.spec.access_log_extern_fields() {
            if let Some(value) = resolve_field(field, session, ctx) {
                let sensitive = should_mask(whitelist, field.from, &field.name);
                ext.insert(&field.key, &value, sensitive);
            }
        }
    });
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
mod tests {
    use super::*;
    use edgion_resources::sensitive::SENSITIVE_MODE;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    // Serialize every test that mutates the global mode.
    static MODE_LOCK: Mutex<()> = Mutex::new(());

    struct ModeGuard<'a> {
        prev: u8,
        _lock: std::sync::MutexGuard<'a, ()>,
    }
    impl Drop for ModeGuard<'_> {
        fn drop(&mut self) {
            SENSITIVE_MODE.store(self.prev, Ordering::SeqCst);
        }
    }
    fn set_mode(mode: u8) -> ModeGuard<'static> {
        let lock = MODE_LOCK.lock().unwrap();
        let prev = SENSITIVE_MODE.swap(mode, Ordering::SeqCst);
        ModeGuard { prev, _lock: lock }
    }

    fn json_of(ext: &LogExt) -> String {
        serde_json::to_string(ext).unwrap()
    }

    #[test]
    fn empty_ext_is_empty() {
        assert!(LogExt::default().is_empty());
    }

    #[test]
    fn non_sensitive_value_is_clear_in_every_mode() {
        for mode in [0u8, 1, 2] {
            let _g = set_mode(mode);
            let mut ext = LogExt::default();
            ext.insert("tenant", "acme", false);
            assert_eq!(json_of(&ext), r#"{"tenant":"acme"}"#, "mode {mode}");
        }
    }

    #[test]
    fn sensitive_value_renders_per_mode() {
        // Mode 2 (wrap) is the only branch where `apply_sensitive` can panic
        // (`try_borrow_mut().expect(...)` in edgion-resources/src/sensitive.rs):
        // `LogExt::serialize` is the only caller that runs a two-step serde op
        // (`serialize_entry` = key + value) inside the sensitive-value closure,
        // exactly the reentrancy shape the doc comment on `Serialize for LogExt`
        // warns about. Exercise it here so that shape stays covered.
        let cases = [
            (0u8, r#"{"session":"abc"}"#),
            (1, r#"{"session":"***"}"#),
            (2, r#"{"session":"<E>abc</E>"}"#),
        ];
        for (mode, expected) in cases {
            let _g = set_mode(mode);
            let mut ext = LogExt::default();
            ext.insert("session", "abc", true);
            assert_eq!(json_of(&ext), expected, "mode {mode}");
        }
    }

    #[test]
    fn later_insert_overwrites_in_place_preserving_order() {
        let _g = set_mode(0);
        let mut ext = LogExt::default();
        ext.insert("a", "1", false);
        ext.insert("b", "2", false);
        // Route tier overwrites the global tier's value for the same key.
        ext.insert("a", "9", false);
        assert_eq!(json_of(&ext), r#"{"a":"9","b":"2"}"#);
        assert_eq!(ext.len(), 2);
    }

    #[test]
    fn overwrite_also_replaces_the_sensitive_flag() {
        let _g = set_mode(1);
        let mut ext = LogExt::default();
        ext.insert("k", "clear", false);
        ext.insert("k", "secret", true);
        assert_eq!(json_of(&ext), r#"{"k":"***"}"#);
    }

    #[test]
    fn value_is_truncated_to_the_cap() {
        let _g = set_mode(0);
        let mut ext = LogExt::default();
        let long = "v".repeat(LOG_LABEL_MAX_VALUE_LEN + 50);
        ext.insert("k", &long, false);
        let json = json_of(&ext);
        // 2 quotes + the key + separators; assert on the value length itself.
        let value = json.trim_start_matches(r#"{"k":""#).trim_end_matches(r#""}"#);
        assert_eq!(value.len(), LOG_LABEL_MAX_VALUE_LEN);
    }

    #[test]
    fn truncation_never_splits_a_utf8_char() {
        let _g = set_mode(0);
        let mut ext = LogExt::default();
        // '€' is 3 bytes, so with an even cap (256) the boundary genuinely
        // straddles a char and the decrement branch actually runs. A 2-byte
        // char like 'é' would make `is_char_boundary(256)` trivially true and
        // never exercise the truncation logic this test is named for.
        let long = "€".repeat(LOG_LABEL_MAX_VALUE_LEN);
        ext.insert("k", &long, false);
        let json = json_of(&ext);
        assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
    }

    #[test]
    fn over_cap_fields_are_dropped() {
        let _g = set_mode(0);
        let mut ext = LogExt::default();
        for i in 0..(LOG_LABEL_MAX_COUNT + 5) {
            ext.insert(&format!("k{i}"), "v", false);
        }
        assert_eq!(ext.len(), LOG_LABEL_MAX_COUNT);
    }

    #[test]
    fn over_cap_still_allows_overwriting_an_existing_key() {
        let _g = set_mode(0);
        let mut ext = LogExt::default();
        for i in 0..LOG_LABEL_MAX_COUNT {
            ext.insert(&format!("k{i}"), "v", false);
        }
        // At the cap: a NEW key is dropped, but an existing key must still update,
        // otherwise the route tier could not override the global tier on a full map.
        ext.insert("new", "v", false);
        ext.insert("k0", "updated", false);
        assert_eq!(ext.len(), LOG_LABEL_MAX_COUNT);
        assert!(json_of(&ext).contains(r#""k0":"updated""#));
    }

    #[test]
    fn attacker_controlled_value_cannot_break_out_of_the_json() {
        let _g = set_mode(0);
        let mut ext = LogExt::default();
        // A header value the client fully controls. serde must escape it; a
        // hand-written `"k":"v"` byte buffer would let this forge entry fields.
        ext.insert("ua", r#"a","status":999,"x":"b"#, false);
        let json = json_of(&ext);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_object().unwrap().len(), 1, "forged a field: {json}");
        assert!(parsed.get("status").is_none(), "forged a field: {json}");
    }

    #[test]
    fn backslash_and_control_chars_are_escaped() {
        let _g = set_mode(0);
        let mut ext = LogExt::default();
        ext.insert("k", "a\\b\"c\nd", false);
        let json = json_of(&ext);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.get("k").unwrap().as_str().unwrap(), "a\\b\"c\nd");
    }

    #[test]
    fn masking_decision_table() {
        use edgion_resources::resources::edgion_gateway_config::UnmaskedKeys;
        use edgion_resources::resources::edgion_plugins::LogExtSource;

        let w = UnmaskedKeys {
            header: vec!["user-agent".to_string()],
            ctx: vec!["tenant_id".to_string()],
            ..Default::default()
        };

        // Category 1 is never masked, whatever the whitelist says.
        assert!(!should_mask(&w, LogExtSource::RouteLabel, "app"));
        assert!(!should_mask(&w, LogExtSource::RouteAnnotation, "owner"));
        // Whitelisted category 2/3 is clear.
        assert!(!should_mask(&w, LogExtSource::Header, "user-agent"));
        assert!(!should_mask(&w, LogExtSource::Ctx, "tenant_id"));
        // Everything else is masked.
        assert!(should_mask(&w, LogExtSource::Header, "x-custom"));
        assert!(should_mask(&w, LogExtSource::Cookie, "session"));
        assert!(should_mask(&w, LogExtSource::Ctx, "other"));
    }
}
