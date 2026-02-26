//! URLRewrite filter implementation
//!
//! This filter rewrites upstream hostname and/or path before forwarding.

use async_trait::async_trait;

use crate::core::plugins::plugin_runtime::log::PluginLog;
use crate::core::plugins::plugin_runtime::traits::{PluginSession, RequestFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::{HTTPPathModifierType, HTTPURLRewriteFilter};

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

        let original_path = session.get_path();
        let query = session.get_query();

        let rewritten_path = match path_modifier.modifier_type {
            HTTPPathModifierType::ReplaceFullPath => path_modifier
                .replace_full_path
                .clone()
                .unwrap_or_else(|| original_path.to_string()),
            HTTPPathModifierType::ReplacePrefixMatch => {
                let replacement = path_modifier
                    .replace_prefix_match
                    .clone()
                    .unwrap_or_else(|| original_path.to_string());

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
                        format!("{}{}", replacement, &original_path[len..])
                    } else {
                        replacement
                    }
                } else {
                    replacement
                }
            }
        };

        let final_uri = match query {
            Some(q) if !q.is_empty() => format!("{}?{}", rewritten_path, q),
            _ => rewritten_path.clone(),
        };

        if let Err(e) = session.set_upstream_uri(&final_uri) {
            plugin_log.push(&format!("URLRewrite path failed: {}; ", e));
        } else {
            plugin_log.push(&format!("URLRewrite path: {} -> {}; ", original_path, rewritten_path));
        }
    }

    fn rewrite_hostname(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        let Some(hostname) = &self.config.hostname else {
            return;
        };

        let mut ok = true;
        if let Err(e) = session.set_upstream_host(hostname) {
            ok = false;
            plugin_log.push(&format!("URLRewrite host failed: {}; ", e));
        }

        // Keep :authority in sync for HTTP/2 style requests.
        let _ = session.set_request_header(":authority", hostname);

        if ok {
            plugin_log.push(&format!("URLRewrite host -> {}; ", hostname));
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
