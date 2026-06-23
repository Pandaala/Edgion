//! GlobalAccessControl runtime plugin (L7 request stage).
//!
//! Resolves the active profile via the Selector EdgionConfigData store (fail-to-base),
//! then evaluates the Plan-1 allow engine against the live request. Unlike IpRestriction,
//! this plugin FAILS OPEN on init/compile error or an unresolvable profile: it allows the
//! request and records the event (access log + tracing). This matches the operator
//! decision that a global allow-list must not take down all traffic on a bad config.

use async_trait::async_trait;
use bytes::Bytes;
use std::net::IpAddr;

use crate::plugins::http::common::auth_common::send_gateway_plain_error_response;
use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
use crate::plugins::runtime::{PluginLog, PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_plugins::plugin_configs::{AllowOutcome, GlobalAccessControlConfig};
use edgion_resources::resources::edgion_plugins::IpSource;

/// GlobalAccessControl L7 allow-list plugin.
pub struct GlobalAccessControl {
    name: String,
    config: GlobalAccessControlConfig,
    /// True when `validate_and_init` failed at construction. Requests then FAIL OPEN
    /// (allowed) rather than be evaluated against uncompiled matchers.
    init_failed: bool,
    /// Resolved "namespace/name" key for the activeProfileRef Selector (built once;
    /// re-looked-up live each request for hot reload). None when no selector configured.
    active_profile_ref_key: Option<String>,
}

impl GlobalAccessControl {
    pub fn create(config: &GlobalAccessControlConfig, namespace: &str) -> Box<dyn RequestFilter> {
        // Matchers (ip_matcher, rule values_set/compiled_regex) are `#[serde(skip)]`, so
        // they are None after gRPC/YAML deserialization; compile them here.
        let mut config = config.clone();
        let init_failed = if let Err(e) = config.validate_and_init() {
            tracing::error!(
                error = %e,
                label = "HTTP plugin",
                "Failed to initialize GlobalAccessControlConfig; failing OPEN (allowing all requests)",
            );
            true
        } else {
            false
        };

        let active_profile_ref_key = config
            .active_profile_ref
            .as_ref()
            .filter(|r| r.ref_denied.is_none())
            .map(|r| {
                let ns = r.namespace.as_deref().unwrap_or(namespace);
                format!("{}/{}", ns, r.name)
            });

        Box::new(GlobalAccessControl {
            name: "GlobalAccessControl".to_string(),
            config,
            init_failed,
            active_profile_ref_key,
        })
    }

    /// Resolve the active profile name via the Selector store, with fail-to-base to
    /// `config.active_profile` on any error/absence. Mirrors the L4 GCIR pattern.
    ///
    /// When a selector IS configured but cannot be honored (EdgionConfigData missing,
    /// kill-switched, wrong variant, or naming an unknown profile), a `selector miss: base`
    /// line is recorded so operators can see the fallback in the access log. A selector
    /// with `active: null` is a deliberate no-op (use base) and is NOT a miss.
    fn resolve_active_profile_name(&self, plugin_log: &mut PluginLog) -> String {
        let Some(ref key) = self.active_profile_ref_key else {
            return self.config.active_profile.clone();
        };
        let store = get_global_edgion_config_data_store();
        let raw_ed = store.get_raw_by_key(key);

        // Envelope kill-switch: spec.enable=false => ignore the selector, use base.
        if raw_ed.as_ref().map(|ed| !ed.spec.enable).unwrap_or(false) {
            plugin_log.push("selector miss: base");
            return self.config.active_profile.clone();
        }
        match raw_ed.as_deref().map(|ed| &ed.spec.data) {
            None => {
                plugin_log.push("selector miss: base");
                self.config.active_profile.clone()
            }
            Some(ConfigEntry::Selector(sel)) => match sel.active.as_deref() {
                // Deliberate no-op (use base) — not a miss.
                None => self.config.active_profile.clone(),
                Some(name) if self.config.profiles.contains_key(name) => name.to_string(),
                Some(_) => {
                    plugin_log.push("selector miss: base");
                    self.config.active_profile.clone()
                }
            },
            Some(_) => {
                plugin_log.push("selector miss: base");
                self.config.active_profile.clone()
            }
        }
    }

    /// Extract the client IP per the configured source. RemoteAddr = TCP peer (un-forgeable).
    fn get_client_ip(&self, session: &dyn PluginSession) -> Option<IpAddr> {
        let ip_str = match self.config.ip_source {
            IpSource::ClientIp => session.remote_addr(),
            IpSource::RemoteAddr => session.client_addr(),
        };
        if ip_str.is_empty() {
            return None;
        }
        ip_str.parse::<IpAddr>().ok()
    }
}

#[async_trait]
impl RequestFilter for GlobalAccessControl {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        // Kill switch.
        if !self.config.enable {
            plugin_log.push("disabled:pass");
            return PluginRunningResult::GoodNext;
        }
        // Fail OPEN on init failure (operator decision): allow + record loudly.
        if self.init_failed {
            plugin_log.push_err("init fail: fail-open");
            return PluginRunningResult::GoodNext;
        }

        let profile_name = self.resolve_active_profile_name(plugin_log);
        let Some(profile) = self.config.profiles.get(&profile_name) else {
            // Post-validation this should be unreachable; fail open if it ever happens.
            plugin_log.push_err("profile missing: fail-open");
            return PluginRunningResult::GoodNext;
        };

        let client_ip = self.get_client_ip(session);

        let outcome = {
            let s: &dyn PluginSession = session;
            profile.evaluate(client_ip.as_ref(), &|rule| {
                crate::plugins::http::common::extract_rule_values(s, rule)
            })
        };

        match outcome {
            AllowOutcome::Allowed { group } => {
                match group {
                    Some(g) => plugin_log.push(&format!("allow:group={}", g)),
                    None => plugin_log.push("allow:default"),
                };
                PluginRunningResult::GoodNext
            }
            AllowOutcome::Denied => {
                plugin_log.push("deny:no-match");
                let message = self.config.message.as_deref().unwrap_or("Access denied");
                let body = Bytes::from(serde_json::json!({ "message": message }).to_string());
                if let Err(_e) =
                    send_gateway_plain_error_response(session, self.config.status, "application/json", body).await
                {
                    plugin_log.push_err("write response fail");
                }
                PluginRunningResult::ErrTerminateRequest
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
    use edgion_resources::resources::edgion_config_data::IpGroup;
    use edgion_resources::resources::edgion_plugins::plugin_configs::{
        AllowProfile, AllowRuleGroup, GlobalAccessControlConfig,
    };
    use edgion_resources::resources::edgion_plugins::plugin_configs::{RestrictionRule, RestrictionSource};
    use edgion_resources::resources::edgion_plugins::DefaultAction;
    use std::collections::HashMap;

    fn ipg(name: &str, cidrs: &[&str]) -> IpGroup {
        IpGroup {
            name: name.into(),
            description: None,
            cidrs: cidrs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn secret_header_rule(key: &str, value: &str) -> RestrictionRule {
        RestrictionRule {
            source: RestrictionSource::Header,
            key: Some(key.into()),
            values: Some(vec![value.into()]),
            ..Default::default()
        }
    }

    // A lockdown config: cloudflare group = CIDR ∧ header. Base "normal" profile = allow-all.
    fn lockdown_config(active_profile_ref: Option<EdgionConfigDataRef>) -> GlobalAccessControlConfig {
        GlobalAccessControlConfig {
            enable: true,
            active_profile: "lockdown".into(),
            profiles: HashMap::from([
                (
                    "normal".to_string(),
                    AllowProfile {
                        default_action: DefaultAction::Allow,
                        rule_groups: vec![],
                    },
                ),
                (
                    "lockdown".to_string(),
                    AllowProfile {
                        default_action: DefaultAction::Deny,
                        rule_groups: vec![AllowRuleGroup {
                            name: Some("cloudflare".into()),
                            ip: Some(vec![ipg("cf", &["173.245.48.0/20"])]),
                            rules: vec![secret_header_rule("X-CF-Secret", "tok")],
                            ip_matcher: None,
                        }],
                    },
                ),
            ]),
            description: None,
            active_profile_ref,
            status: 403,
            message: None,
            ip_source: edgion_resources::resources::edgion_plugins::IpSource::RemoteAddr,
        }
    }

    #[tokio::test]
    async fn allow_when_cidr_and_secret_match() {
        let plugin = GlobalAccessControl::create(&lockdown_config(None), "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("173.245.48.5".to_string());
        s.expect_header_value().returning(|name| {
            if name == "X-CF-Secret" {
                Some("tok".to_string())
            } else {
                None
            }
        });
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("allow:group=cloudflare"));
    }

    #[tokio::test]
    async fn deny_when_cidr_matches_but_secret_absent() {
        let plugin = GlobalAccessControl::create(&lockdown_config(None), "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("173.245.48.5".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        assert!(log.contains("deny:no-match"));
    }

    #[tokio::test]
    async fn deny_when_secret_matches_but_ip_outside_cidr() {
        let plugin = GlobalAccessControl::create(&lockdown_config(None), "edgion-system");
        let mut s = MockPluginSession::new();
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_header_value().returning(|name| {
            if name == "X-CF-Secret" {
                Some("tok".to_string())
            } else {
                None
            }
        });
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
    }

    #[tokio::test]
    async fn disabled_passes_through() {
        let mut cfg = lockdown_config(None);
        cfg.enable = false;
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("disabled:pass"));
    }

    #[tokio::test]
    async fn init_failure_fails_open() {
        // A config with an invalid CIDR fails validate_and_init at construction.
        let mut cfg = lockdown_config(None);
        cfg.profiles.get_mut("lockdown").unwrap().rule_groups[0].ip = Some(vec![ipg("bad", &["not-a-cidr"])]);
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        // Fail-OPEN: allow + record.
        assert_eq!(r, PluginRunningResult::GoodNext);
        assert!(log.contains("init fail: fail-open"));
    }

    #[tokio::test]
    async fn unresolved_selector_falls_back_to_base_active_profile() {
        // active_profile_ref points at an EdgionConfigData not present in the global store
        // (empty in unit tests) -> fail-to-base to active_profile "lockdown".
        let cfg = lockdown_config(Some(EdgionConfigDataRef {
            name: "ghost".into(),
            namespace: Some("edgion-system".into()),
            ..Default::default()
        }));
        let plugin = GlobalAccessControl::create(&cfg, "edgion-system");
        let mut s = MockPluginSession::new();
        // lockdown base: IP outside + no header -> deny.
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(r, PluginRunningResult::ErrTerminateRequest);
        // The unresolvable selector (not in the empty store) must be recorded.
        assert!(log.contains("selector miss: base"));
    }

    /// A denied active_profile_ref must not be resolved as a store key — even if a
    /// valid Selector at that key would switch to the "normal" (allow-all) profile.
    /// Effect: base "lockdown" profile is used (deny-all for 8.8.8.8).
    #[tokio::test]
    async fn denied_active_profile_ref_falls_back_to_base() {
        use crate::plugins::runtime::edgion_config_data::get_global_edgion_config_data_store;
        use edgion_resources::resources::edgion_config_data::{
            ConfigEntry, EdgionConfigData, EdgionConfigDataSpec, Selector,
        };
        use std::sync::Arc as StdArc;

        // Seed a valid Selector that WOULD switch to "normal" (allow-all).
        let ns = "gac-ref-denied-unit";
        let sel_key = format!("{ns}/sel-denied");
        let mut sel_ed = EdgionConfigData::new(
            "sel-denied",
            EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::Selector(Selector {
                    active: Some("normal".to_string()),
                    description: None,
                }),
            },
        );
        sel_ed.metadata.namespace = Some(ns.to_string());
        get_global_edgion_config_data_store().insert_raw_for_test(&sel_key, StdArc::new(sel_ed));

        // Config points at the Selector but marks it as denied.
        let cfg = lockdown_config(Some(EdgionConfigDataRef {
            name: "sel-denied".into(),
            namespace: Some(ns.into()),
            ref_denied: Some(edgion_resources::resources::common::RefDenied {
                target_namespace: ns.into(),
                target_name: "sel-denied".into(),
                reason: None,
            }),
        }));
        let plugin = GlobalAccessControl::create(&cfg, ns);
        let mut s = MockPluginSession::new();
        // With denied ref → active_profile_ref_key is None → base "lockdown" used → deny.
        s.expect_client_addr().return_const("8.8.8.8".to_string());
        s.expect_header_value().returning(|_| None);
        s.expect_write_response_header().returning(|_, _| Ok(()));
        s.expect_write_response_body().returning(|_, _| Ok(()));
        s.expect_shutdown().returning(|| {});
        let mut log = PluginLog::new("GlobalAccessControl");
        let r = plugin.run_request(&mut s, &mut log).await;
        assert_eq!(
            r,
            PluginRunningResult::ErrTerminateRequest,
            "denied active_profile_ref must not switch to normal (allow-all); lockdown must deny"
        );
        // No "selector miss" emitted — the key was never looked up (denied at construction).
        assert!(
            !log.contains("selector miss: base"),
            "denied ref must not trigger a store lookup"
        );
    }
}
