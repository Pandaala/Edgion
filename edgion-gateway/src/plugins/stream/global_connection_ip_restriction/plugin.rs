//! GlobalConnectionIpRestriction stream plugin — L4 pre-TLS IP filter with an active-profile selector.
//!
//! # Model
//! The plugin holds an inline, pre-compiled `base` config (the flat
//! `GlobalConnectionIpRestrictionConfig` from the stream-plugin spec). It carries the
//! complete, git-owned IP rule data (`enable` + named `profiles`). The only dynamic input
//! is *which profile is active*: when `active_profile_ref` is set, the plugin looks up a
//! `Selector` EdgionConfigData at connection time and uses its `active` profile name —
//! but only to choose among the base's own profiles. The rule data is never overlaid.
//!
//! # Fail-to-base semantics (signals via Prometheus, never `tracing` on the data plane)
//! - **`overlay_missing`**: `activeProfileRef` set but EdgionConfigData not found → use base.
//! - **`overlay_type_mismatch`**: EdgionConfigData present but not a `Selector` variant → use base.
//! - **`overlay_disabled`**: EdgionConfigData envelope `spec.enable=false` → use base.
//! - **`overlay_invalid`**: Selector names a profile not in `profiles` → use base.
//! - Selector `active: null` / absent → **no-op**, use base (no counter).
//!
//! A selector can only switch among the git-owned `profiles`; it can never disable filtering
//! or inject rules. Every non-switch outcome falls back to `active_profile` (fail-to-base),
//! so the base MUST be authored as the conservative/safe value (e.g. `defaultAction: Deny`).
//!
//! # Geo (country) second stage and PROXY protocol
//! Each profile may carry an optional `geo` block (`GeoRules`) evaluated after the IP
//! stage. `StreamContext.client_ip` is the raw transport peer at stage-1 (PROXY protocol
//! parsing happens in `handshake()`, strictly after `accept()`) and the resolved client
//! at the TCP/TLS stages. This is a pre-existing property — the stage-1 **IP** rules
//! already match the transport peer today, not the PROXY-declared client — and geo
//! deliberately inherits it rather than diverging. An operator enabling PROXY protocol
//! owns the consequence.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::geoip::{GeoIpProvider, GeoLookup};
use crate::observe::metrics::{record_conn_ip_restriction_event, record_geoip_denied};
use crate::plugins::runtime::edgion_config_data::{
    get_global_edgion_config_data_store, resolve_ip_list_matchers, resolve_ip_list_ref_keys,
};
use crate::plugins::stream::stream_plugin_trait::{
    access_decision_to_stream_result, StreamContext, StreamPlugin, StreamPluginResult,
};
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::AccessDecision;
use edgion_resources::resources::edgion_stream_plugins::GlobalConnectionIpRestrictionConfig;

const PLUGIN_NAME: &str = "GlobalConnectionIpRestriction";

pub struct GlobalConnectionIpRestrictionPlugin {
    /// Pre-compiled base config. Matchers are compiled at construction time via
    /// `validate_and_init`. Used directly when no selector is set, and as the
    /// fail-to-base fallback on any selector error.
    base: Arc<GlobalConnectionIpRestrictionConfig>,
    /// Optional EdgionConfigData store key ("namespace/name") for a `Selector` that
    /// supplies the active profile name at connection time. `None` means no selector.
    active_profile_ref_key: Option<String>,
    /// Per-profile resolved IpList ref keys (deny_keys, allow_keys), keyed by profile name.
    /// Pre-resolved at construction because `parent_namespace` is not retained.
    profile_ref_keys: HashMap<String, (Vec<String>, Vec<String>)>,
}

impl GlobalConnectionIpRestrictionPlugin {
    /// Construct a plugin from a `GlobalConnectionIpRestrictionConfig`.
    ///
    /// Calls `validate_and_init` on the config to pre-compile IP matchers.
    /// Returns an error if the config is structurally invalid (this should not happen in
    /// practice if the controller validated before syncing).
    pub fn new(config: &GlobalConnectionIpRestrictionConfig, parent_namespace: &str) -> Result<Self, String> {
        let mut base = config.clone();
        base.validate_and_init()
            .map_err(|e| format!("GlobalConnectionIpRestriction config validation failed: {e}"))?;

        let active_profile_ref_key = config
            .active_profile_ref
            .as_ref()
            .filter(|r| r.ref_denied.is_none())
            .map(|r| {
                let ns = r.namespace.as_deref().unwrap_or(parent_namespace);
                format!("{}/{}", ns, r.name)
            });

        let profile_ref_keys = config
            .profiles
            .iter()
            .map(|(name, p)| {
                let deny_keys = resolve_ip_list_ref_keys(p.deny_refs.as_ref(), parent_namespace);
                let allow_keys = resolve_ip_list_ref_keys(p.allow_refs.as_ref(), parent_namespace);
                (name.clone(), (deny_keys, allow_keys))
            })
            .collect();

        Ok(Self {
            base: Arc::new(base),
            active_profile_ref_key,
            profile_ref_keys,
        })
    }

    /// Resolve the effective active-profile **name** for this connection.
    ///
    /// A selector can only choose another git-owned profile, so this returns just the profile
    /// name — no rule-data clone on the hot path. Every error / `active: null` / disabled path
    /// returns the base default (`active_profile`), i.e. fail-to-base. The caller indexes
    /// `self.base.profiles` with the returned name (matchers already compiled at construction).
    fn resolve_active_profile(&self) -> String {
        let Some(ref key) = self.active_profile_ref_key else {
            // No selector configured — use the git default.
            return self.base.active_profile.clone();
        };

        let store = get_global_edgion_config_data_store();
        let raw_ed = store.get_raw_by_key(key);

        // Envelope kill-switch: spec.enable=false means the operator disabled this
        // EdgionConfigData entirely. Ignore the selector; use base.
        if raw_ed.as_ref().map(|ed| !ed.spec.enable).unwrap_or(false) {
            record_conn_ip_restriction_event("overlay_disabled");
            return self.base.active_profile.clone();
        }

        match raw_ed.as_deref().map(|ed| &ed.spec.data) {
            None => {
                // activeProfileRef set but EdgionConfigData not found in the store.
                record_conn_ip_restriction_event("overlay_missing");
                self.base.active_profile.clone()
            }
            Some(ConfigEntry::Selector(sel)) => match sel.active.as_deref() {
                // `active: null` / absent → deliberate no-op (use base).
                None => self.base.active_profile.clone(),
                // Valid switch among the git-owned profiles.
                Some(name) if self.base.profiles.contains_key(name) => name.to_string(),
                // Selector names a profile that does not exist in the git-owned profiles.
                Some(_) => {
                    record_conn_ip_restriction_event("overlay_invalid");
                    self.base.active_profile.clone()
                }
            },
            Some(_) => {
                // EdgionConfigData found but it is not a Selector variant.
                record_conn_ip_restriction_event("overlay_type_mismatch");
                self.base.active_profile.clone()
            }
        }
    }
}

#[async_trait]
impl StreamPlugin for GlobalConnectionIpRestrictionPlugin {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult {
        // Kill switch (git-owned; a selector cannot touch it) — allow all connections.
        if !self.base.enable {
            return StreamPluginResult::Allow;
        }

        let active = self.resolve_active_profile();
        let Some(profile) = self.base.profiles.get(&active) else {
            // Should not happen after validate_and_init + membership check, but guard defensively.
            record_conn_ip_restriction_event("fail_closed_profile_missing");
            return StreamPluginResult::Deny("ip:deny:profile-missing".to_string());
        };

        static NO_KEYS: (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        let (deny_keys, allow_keys) = self.profile_ref_keys.get(&active).unwrap_or(&NO_KEYS);
        let deny_arcs = resolve_ip_list_matchers(deny_keys);
        let allow_arcs = resolve_ip_list_matchers(allow_keys);
        let deny_refs: Vec<&IpRadixMatcher> = deny_arcs.iter().map(|a| a.as_ref()).collect();
        let allow_refs: Vec<&IpRadixMatcher> = allow_arcs.iter().map(|a| a.as_ref()).collect();
        let ip_decision = profile.check_ip_with_refs(&ctx.client_ip, &deny_refs, &allow_refs);

        let Some(geo) = profile.geo.as_ref() else {
            // No geo block: today's behavior, byte for byte.
            return access_decision_to_stream_result(ip_decision, || record_conn_ip_restriction_event("denied"));
        };

        match &ip_decision {
            // Row 1 and row 2 are terminal and bypass geo entirely.
            AccessDecision::DenyByGroup(_) => {
                return access_decision_to_stream_result(ip_decision, || record_conn_ip_restriction_event("denied"))
            }
            AccessDecision::AllowByGroup(_) => return StreamPluginResult::Allow,
            // Rows 3 and 4: the IP stage had no verdict of its own.
            AccessDecision::DenyByWhitelistMiss | AccessDecision::Default(_) => {}
        }

        let layer = ctx.layer;
        let lookup = GeoIpProvider::global().lookup(ctx.client_ip);

        // Row 3a. `Unavailable` (total outage) and `NotFound` (address absent
        // from a loaded database) are deliberately NOT the same path: a
        // fail-open outage must return immediately with the original IP
        // decision (skip 3b/3c entirely, go straight to row 4), never fall
        // through into the allow-mode-miss handling below — that handling is
        // correct only for a resolved-but-unlisted country (`NotFound`), and
        // conflating the two would silently defeat `failOpen: true` whenever
        // `allowCountries` is configured (an outage would deny instead of
        // deferring to `ip_decision`).
        let country = match &lookup {
            GeoLookup::Unavailable => {
                if !geo.fail_open {
                    record_geoip_denied(layer, "db_unavailable");
                    return StreamPluginResult::Deny("geo:deny:db-unavailable".to_string());
                }
                return access_decision_to_stream_result(ip_decision, || record_conn_ip_restriction_event("denied"));
            }
            GeoLookup::NotFound => None,
            GeoLookup::Found(info) => info.country_iso.clone(),
        };

        if let Some(c) = country.as_deref() {
            // Row 3b.
            if geo.denies_country(c) {
                record_geoip_denied(layer, "deny_country");
                return StreamPluginResult::Deny("geo:deny:country".to_string());
            }
            // Row 3c — terminal in BOTH directions.
            if !geo.allow_countries.is_empty() {
                return if geo.allows_country(c) {
                    StreamPluginResult::Allow
                } else {
                    record_geoip_denied(layer, "allow_miss");
                    StreamPluginResult::Deny("geo:deny:allow-miss".to_string())
                };
            }
        } else if !geo.allow_countries.is_empty() {
            // An allowlist is configured but we have no country to check it
            // against: an allow-mode miss, not a fall-through.
            record_geoip_denied(layer, "allow_miss");
            return StreamPluginResult::Deny("geo:deny:allow-miss".to_string());
        }

        // Row 3d: geo has no opinion. Rows 4 / 4' — hand the ORIGINAL IP
        // decision back untouched.
        //
        // This is load-bearing: `DenyByWhitelistMiss` must stay a denial. Geo
        // must never let a whitelist-mode miss reach `default_action`, whose
        // default value is Allow.
        access_decision_to_stream_result(ip_decision, || record_conn_ip_restriction_event("denied"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
    use edgion_resources::resources::edgion_config_data::{
        ConfigEntry, EdgionConfigData, EdgionConfigDataRef, EdgionConfigDataSpec, IpGroup, IpListData, Selector,
    };
    use edgion_resources::resources::edgion_plugins::DefaultAction;
    use edgion_resources::resources::edgion_stream_plugins::{GlobalConnectionIpRestrictionConfig, ProfileRules};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn ip_group(name: &str, cidr: &str) -> IpGroup {
        IpGroup {
            name: name.to_string(),
            description: None,
            cidrs: vec![cidr.to_string()],
        }
    }

    /// One profile that allows only `allow_cidr` (default deny), or with a deny entry.
    fn profile(default_action: DefaultAction, allow_cidr: Option<&str>, deny_cidr: Option<&str>) -> ProfileRules {
        ProfileRules {
            allow: allow_cidr.map(|c| vec![ip_group("test-allow", c)]),
            deny: deny_cidr.map(|c| vec![ip_group("test-deny", c)]),
            default_action,
            allow_refs: None,
            deny_refs: None,
            geo: None,
            allow_matcher: None,
            deny_matcher: None,
        }
    }

    /// Extend `profile` with a geo block (pass `None` for today's shape).
    fn profile_with_geo(
        default_action: DefaultAction,
        allow_cidr: Option<&str>,
        deny_cidr: Option<&str>,
        geo: Option<edgion_resources::resources::edgion_stream_plugins::GeoRules>,
    ) -> ProfileRules {
        let mut p = profile(default_action, allow_cidr, deny_cidr);
        p.geo = geo;
        p
    }

    fn geo(
        allow: &[&str],
        deny: &[&str],
        fail_open: bool,
    ) -> edgion_resources::resources::edgion_stream_plugins::GeoRules {
        let mut g = edgion_resources::resources::edgion_stream_plugins::GeoRules {
            allow_countries: allow.iter().map(|s| s.to_string()).collect(),
            deny_countries: deny.iter().map(|s| s.to_string()).collect(),
            fail_open,
        };
        g.compile().unwrap();
        g
    }

    fn plugin_with(profile: ProfileRules) -> GlobalConnectionIpRestrictionPlugin {
        let mut profiles = HashMap::new();
        profiles.insert("main".to_string(), profile);
        let config = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "main".to_string(),
            profiles,
            description: None,
            active_profile_ref: None,
        };
        GlobalConnectionIpRestrictionPlugin::new(&config, "default").expect("valid config")
    }

    /// Synchronous on purpose: the geo tests below hold `GEO_TEST_LOCK` across
    /// this call to serialize access to the process-global `GeoIpProvider`
    /// singleton for the test's full body. A `parking_lot::MutexGuard` must
    /// never live across a real `.await` suspension point (workspace lint
    /// `await_holding_lock`), so this drives `on_connection`'s future to
    /// completion with a plain synchronous poll loop instead of `#[tokio::test]`
    /// + `.await` — `on_connection` itself never actually suspends.
    fn decide(profile: ProfileRules, client: &str) -> StreamPluginResult {
        let plugin = plugin_with(profile);
        let ctx = StreamContext::new(client.parse().unwrap(), 8443);
        futures::executor::block_on(plugin.on_connection(&ctx))
    }

    fn is_allow(r: &StreamPluginResult) -> bool {
        matches!(r, StreamPluginResult::Allow)
    }

    fn load_fixture() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/geoip/edgion-test-city.mmdb");
        crate::geoip::GeoIpProvider::global().load_from(&path).unwrap();
    }

    /// Build a single-profile config (no selector ref).
    fn make_config(
        active_profile: &str,
        default_action: DefaultAction,
        allow_cidr: Option<&str>,
        deny_cidr: Option<&str>,
    ) -> GlobalConnectionIpRestrictionConfig {
        GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: active_profile.to_string(),
            profiles: HashMap::from([(
                active_profile.to_string(),
                profile(default_action, allow_cidr, deny_cidr),
            )]),
            description: None,
            active_profile_ref: None,
        }
    }

    fn plugin_deny_nonloopback() -> GlobalConnectionIpRestrictionPlugin {
        let config = make_config("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
        GlobalConnectionIpRestrictionPlugin::new(&config, "default").expect("valid config")
    }

    /// Insert a raw EdgionConfigData into the store under "ns/name".
    fn insert_ed(ns: &str, name: &str, enable: bool, data: ConfigEntry) -> String {
        let key = format!("{ns}/{name}");
        let mut ed = EdgionConfigData::new(
            name,
            EdgionConfigDataSpec {
                enable,
                active: None,
                visibility: Default::default(),
                data,
            },
        );
        ed.metadata.namespace = Some(ns.to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(&key, Arc::new(ed));
        key
    }

    #[tokio::test]
    async fn no_selector_base_allow_loopback() {
        let plugin = plugin_deny_nonloopback();
        let ctx = StreamContext::new("127.0.0.1".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
    }

    #[tokio::test]
    async fn no_selector_base_deny_non_loopback() {
        let plugin = plugin_deny_nonloopback();
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
    }

    #[tokio::test]
    async fn absent_selector_falls_back_to_base() {
        let mut config = make_config("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
        config.active_profile_ref = Some(EdgionConfigDataRef {
            name: "gcir-unit-absent-selector-do-not-register".to_string(),
            namespace: Some("default".to_string()),
            ..Default::default()
        });
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, "default").expect("valid config");

        let ctx = StreamContext::new("127.0.0.1".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
        let ctx2 = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx2).await, StreamPluginResult::Deny(_)));
    }

    #[tokio::test]
    async fn enable_false_allows_all_connections() {
        let mut config = make_config("main", DefaultAction::Deny, Some("127.0.0.0/8"), None);
        config.enable = false;
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, "default").expect("valid config");
        let ctx = StreamContext::new("1.2.3.4".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
    }

    /// Build a config with two profiles: "deny" (allow loopback only) and "open" (allow all),
    /// active "deny" by default, referencing a Selector under `ref_key`.
    fn two_profile_config(ns: &str, ref_name: &str) -> GlobalConnectionIpRestrictionConfig {
        GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "deny".to_string(),
            profiles: HashMap::from([
                (
                    "deny".to_string(),
                    profile(DefaultAction::Deny, Some("127.0.0.0/8"), None),
                ),
                (
                    "open".to_string(),
                    profile(DefaultAction::Deny, Some("0.0.0.0/0"), None),
                ),
            ]),
            description: None,
            active_profile_ref: Some(EdgionConfigDataRef {
                name: ref_name.to_string(),
                namespace: Some(ns.to_string()),
                ..Default::default()
            }),
        }
    }

    #[tokio::test]
    async fn selector_switches_active_profile() {
        let ns = "unit-sel-switch";
        insert_ed(
            ns,
            "sel-open",
            true,
            ConfigEntry::Selector(Selector {
                active: Some("open".to_string()),
                description: None,
            }),
        );
        let config = two_profile_config(ns, "sel-open");
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
        // Selector switches active profile to "open" → 8.8.8.8 is allowed.
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Allow));
    }

    #[tokio::test]
    async fn selector_active_null_is_noop_uses_base() {
        let ns = "unit-sel-null";
        insert_ed(
            ns,
            "sel-null",
            true,
            ConfigEntry::Selector(Selector {
                active: None,
                description: Some("intentional no-op".to_string()),
            }),
        );
        let config = two_profile_config(ns, "sel-null");
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
        // active: null → no override → base "deny" profile → 8.8.8.8 denied.
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
    }

    #[tokio::test]
    async fn selector_unknown_profile_falls_back_to_base() {
        let ns = "unit-sel-unknown";
        insert_ed(
            ns,
            "sel-bad",
            true,
            ConfigEntry::Selector(Selector {
                active: Some("does-not-exist".to_string()),
                description: None,
            }),
        );
        let config = two_profile_config(ns, "sel-bad");
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
        // Unknown profile → fail-to-base → base "deny" → 8.8.8.8 denied.
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
    }

    #[tokio::test]
    async fn type_mismatch_falls_back_to_base() {
        let ns = "unit-sel-mismatch";
        // A non-Selector EdgionConfigData (IpList) at the referenced key.
        insert_ed(
            ns,
            "not-a-selector",
            true,
            ConfigEntry::IpList(IpListData { items: vec![] }),
        );
        let config = two_profile_config(ns, "not-a-selector");
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
        // Wrong variant → fail-to-base → base "deny" → 8.8.8.8 denied.
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
    }

    /// Envelope kill-switch: a Selector that WOULD switch to "open" is ignored because the
    /// EdgionConfigData envelope `spec.enable=false`. Base ("deny") is used.
    #[tokio::test]
    async fn envelope_disabled_falls_back_to_base() {
        let ns = "unit-sel-envelope";
        insert_ed(
            ns,
            "sel-envelope-off",
            false, // envelope kill-switch
            ConfigEntry::Selector(Selector {
                active: Some("open".to_string()),
                description: None,
            }),
        );
        let config = two_profile_config(ns, "sel-envelope-off");
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
        // Envelope disabled → selector ignored → base "deny" → 8.8.8.8 denied.
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)));
        // Loopback still allowed by base.
        let ctx_lo = StreamContext::new("127.0.0.1".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx_lo).await, StreamPluginResult::Allow));
    }

    #[test]
    fn invalid_base_returns_error() {
        // active_profile not present in profiles → construction error.
        let config = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "nonexistent".to_string(),
            profiles: HashMap::from([("p1".to_string(), profile(DefaultAction::Deny, Some("10.0.0.0/8"), None))]),
            description: None,
            active_profile_ref: None,
        };
        assert!(GlobalConnectionIpRestrictionPlugin::new(&config, "default").is_err());
    }

    /// A denied active_profile_ref must not be resolved as a store key (ref_denied is
    /// set) — even if a Selector at that key would switch to the "open" profile.
    /// Effect: base "deny" profile is used (8.8.8.8 is denied instead of allowed).
    #[tokio::test]
    async fn gcir_denied_active_profile_ref_falls_back_to_base() {
        let ns = "gcir-ref-denied-unit";
        // Seed a valid Selector that WOULD switch the active profile to "open".
        insert_ed(
            ns,
            "sel-denied",
            true,
            ConfigEntry::Selector(edgion_resources::resources::edgion_config_data::Selector {
                active: Some("open".to_string()),
                description: None,
            }),
        );
        // Build a two-profile config pointing at that selector, but with ref_denied set.
        let mut config = two_profile_config(ns, "sel-denied");
        config.active_profile_ref.as_mut().unwrap().ref_denied = Some(edgion_resources::resources::common::RefDenied {
            target_namespace: ns.to_string(),
            target_name: "sel-denied".to_string(),
            reason: None,
        });
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid config");
        // Denied ref → active_profile_ref_key is None → base "deny" profile used.
        // 8.8.8.8 must be denied (not allowed by "open").
        let ctx = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(
            matches!(plugin.on_connection(&ctx).await, StreamPluginResult::Deny(_)),
            "denied active_profile_ref must not switch profile; base deny must apply"
        );
    }

    #[tokio::test]
    async fn profile_deny_refs_block_referenced_ip() {
        let ns = "gcir-denyref-unit";
        // Seed a deny IpList.
        insert_ed(
            ns,
            "blk",
            true,
            ConfigEntry::IpList(IpListData {
                items: vec![ip_group("blocked", "9.9.9.9/32")],
            }),
        );
        // One profile, default Allow, with a denyRef to the seeded IpList.
        let mut profile = profile(DefaultAction::Allow, None, None);
        profile.deny_refs = Some(vec![EdgionConfigDataRef {
            name: "blk".into(),
            namespace: Some(ns.into()),
            ..Default::default()
        }]);
        let config = GlobalConnectionIpRestrictionConfig {
            enable: true,
            active_profile: "main".to_string(),
            profiles: HashMap::from([("main".to_string(), profile)]),
            description: None,
            active_profile_ref: None,
        };
        let plugin = GlobalConnectionIpRestrictionPlugin::new(&config, ns).expect("valid");
        let ctx_block = StreamContext::new("9.9.9.9".parse().unwrap(), 8080);
        assert!(matches!(
            plugin.on_connection(&ctx_block).await,
            StreamPluginResult::Deny(_)
        ));
        let ctx_ok = StreamContext::new("8.8.8.8".parse().unwrap(), 8080);
        assert!(matches!(plugin.on_connection(&ctx_ok).await, StreamPluginResult::Allow));
    }

    // ---- Task 14: geo second stage ----
    //
    // The addresses below come from the fixture map: `203.0.113.7` resolves to
    // GB, `127.0.0.5` to RU, `127.0.0.6` to US, `127.0.0.7` to HK, `127.0.0.1`
    // to CN. `GeoIpProvider::global()` is a process-global singleton, so every
    // test here takes `GEO_TEST_LOCK` before touching it and holds it for the
    // whole test body (including the `decide()` call) to serialize against
    // every other test that touches the same singleton. These are plain
    // `#[test]` functions (not `#[tokio::test]`) specifically so that guard
    // never spans a real `.await` suspension point — see `decide`'s doc comment.

    /// A profile with no geo block must behave EXACTLY as it does today.
    #[test]
    fn no_geo_block_preserves_existing_behavior() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        let p = profile_with_geo(DefaultAction::Allow, Some("10.0.0.0/8"), None, None);
        // Whitelist mode: an out-of-allowlist address is denied, and the inert
        // `Allow` default is never consulted.
        assert!(!is_allow(&decide(p, "203.0.113.5")));
    }

    /// Row 1: IP deny wins over everything, including an allowed country.
    #[test]
    fn ip_deny_hit_rejects_before_geo() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture();
        let p = profile_with_geo(
            DefaultAction::Allow,
            None,
            Some("203.0.113.0/24"),
            Some(geo(&["GB"], &[], false)),
        );
        assert!(!is_allow(&decide(p, "203.0.113.7")));
    }

    /// Row 2: an IP allowlist hit exempts the connection from a country blacklist.
    /// This exemption is the expressive value of merging geo into this plugin.
    #[test]
    fn ip_allow_hit_exempts_a_geo_deny() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture();
        let p = profile_with_geo(
            DefaultAction::Deny,
            Some("203.0.113.0/24"),
            None,
            Some(geo(&[], &["GB"], false)),
        );
        assert!(is_allow(&decide(p, "203.0.113.7")));
    }

    /// Row 4 — THE REGRESSION THIS TASK EXISTS TO PREVENT.
    ///
    /// `DefaultAction::default()` is `Allow`, so nearly every whitelist profile
    /// carries an inert `Allow`. If a geo no-opinion fell through to it, an
    /// operator adding `denyCountries` to a working IP whitelist — intending to
    /// block MORE — would instead admit every address outside their allowlist.
    #[test]
    fn whitelist_miss_still_denies_when_geo_has_no_opinion() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture();
        let p = profile_with_geo(
            DefaultAction::Allow, // the inert default
            Some("10.0.0.0/8"),   // whitelist mode
            None,
            Some(geo(&[], &["RU"], true)), // deny-only: no opinion on GB
        );
        // 203.0.113.7 resolves to GB, is not in 10/8, and is not RU.
        assert!(
            !is_allow(&decide(p, "203.0.113.7")),
            "a geo no-opinion must not resurrect default_action in whitelist mode"
        );
    }

    /// Row 3c: an `allowCountries` hit must be terminal. If it fell through to
    /// `default_action`, then in any `defaultAction: Deny` profile — the
    /// conservative baseline this plugin mandates — `allowCountries` would admit
    /// nobody.
    #[test]
    fn allow_countries_hit_admits_under_default_deny() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture();
        let p = profile_with_geo(DefaultAction::Deny, None, None, Some(geo(&["GB"], &[], false)));
        assert!(is_allow(&decide(p, "203.0.113.7")));
    }

    #[test]
    fn allow_countries_miss_denies() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture();
        let p = profile_with_geo(DefaultAction::Allow, None, None, Some(geo(&["CN"], &[], false)));
        assert!(!is_allow(&decide(p, "203.0.113.7")));
    }

    #[test]
    fn deny_countries_hit_denies() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        load_fixture();
        let p = profile_with_geo(DefaultAction::Allow, None, None, Some(geo(&[], &["GB"], false)));
        assert!(!is_allow(&decide(p, "203.0.113.7")));
    }

    /// Row 3a, fail-closed: no database at all denies.
    #[test]
    fn db_unavailable_denies_when_fail_open_is_false() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        crate::geoip::GeoIpProvider::global().clear();
        let p = profile_with_geo(DefaultAction::Allow, None, None, Some(geo(&[], &["RU"], false)));
        assert!(!is_allow(&decide(p, "203.0.113.7")));
    }

    /// Row 3a, fail-open: no database behaves as if the geo block were absent.
    #[test]
    fn db_unavailable_falls_through_when_fail_open_is_true() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        crate::geoip::GeoIpProvider::global().clear();
        let p = profile_with_geo(DefaultAction::Allow, None, None, Some(geo(&[], &["RU"], true)));
        assert!(is_allow(&decide(p, "203.0.113.7")));
    }

    /// Regression test for the reviewer-caught CRITICAL: `Unavailable` +
    /// `failOpen: true` must skip 3b/3c entirely and defer to the original
    /// `ip_decision`, even when `allowCountries` is configured. Before the
    /// fix, `Unavailable` mapped `country` to `None` and fell through into
    /// the SAME branch that handles a resolved-but-unlisted country
    /// (`NotFound`), which unconditionally denies whenever an allowlist is
    /// configured — silently defeating `failOpen: true` for exactly this
    /// configuration (an `allowCountries`-only profile with the implicit
    /// `defaultAction: Allow`). No `db_unavailable_*` test above catches this:
    /// they are all deny-only geo blocks, so `allow_countries` is empty and
    /// the buggy branch is never entered.
    #[test]
    fn db_unavailable_with_allow_countries_and_fail_open_still_allows() {
        let _g = crate::geoip::GEO_TEST_LOCK.lock();
        crate::geoip::GeoIpProvider::global().clear();
        let p = profile_with_geo(DefaultAction::Allow, None, None, Some(geo(&["GB"], &[], true)));
        assert!(
            is_allow(&decide(p, "203.0.113.7")),
            "failOpen: true must defer to the original ip_decision (Default(Allow)) \
             on a total outage, even with allowCountries configured"
        );
    }
}
