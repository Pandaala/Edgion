//! URLRewrite filter implementation
//!
//! This filter rewrites upstream hostname and/or path before forwarding.

use async_trait::async_trait;

use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{HTTPPathModifierType, HTTPURLRewriteFilter};

pub struct URLRewriteFilter {
    config: HTTPURLRewriteFilter,
}

impl URLRewriteFilter {
    pub fn new(config: HTTPURLRewriteFilter) -> Self {
        Self { config }
    }

    fn rewrite_path(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let Some(path_modifier) = &self.config.path else {
            return;
        };

        let original_path = session.get_path().to_string();
        let query = session.get_query().map(|q| q.to_string());

        let rewritten_path = match path_modifier.modifier_type {
            HTTPPathModifierType::ReplaceFullPath => path_modifier
                .replace_full_path
                .clone()
                .unwrap_or_else(|| original_path.clone()),
            HTTPPathModifierType::ReplacePrefixMatch => {
                let replacement = path_modifier
                    .replace_prefix_match
                    .clone()
                    .unwrap_or_else(|| original_path.clone());

                // Prefer matched route prefix length when available.
                let matched_prefix_len = session
                    .ctx()
                    .route_unit
                    .as_ref()
                    .and_then(|unit| unit.matched_info.m.path.as_ref())
                    .and_then(|path_match| {
                        if path_match.match_type.as_deref() == Some("PathPrefix") {
                            path_match.value.as_ref().map(|v| v.len())
                        } else {
                            None
                        }
                    });

                if let Some(len) = matched_prefix_len {
                    if len <= original_path.len() {
                        let suffix = &original_path[len..];
                        if replacement.ends_with('/') && suffix.starts_with('/') {
                            format!("{}{}", replacement, &suffix[1..])
                        } else {
                            format!("{}{}", replacement, suffix)
                        }
                    } else {
                        replacement
                    }
                } else {
                    replacement
                }
            }
        };

        let final_uri = match &query {
            Some(q) if !q.is_empty() => format!("{}?{}", rewritten_path, q),
            _ => rewritten_path.clone(),
        };

        if let Err(_e) = session.set_upstream_uri(&final_uri) {
            plugin_log.push_err("path rewrite fail");
        } else {
            plugin_log.push("path rewrite");
        }
    }

    fn rewrite_hostname(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let Some(hostname) = &self.config.hostname else {
            return;
        };

        if let Err(_e) = session.set_upstream_host(hostname) {
            plugin_log.push_err("host rewrite fail");
        } else {
            plugin_log.push("host rewrite");
        }
    }
}

#[async_trait]
impl RequestFilter for URLRewriteFilter {
    fn name(&self) -> &str {
        "URLRewrite"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        self.rewrite_hostname(session, plugin_log);
        self.rewrite_path(session, plugin_log);
        PluginRunningResult::GoodNext
    }
}
