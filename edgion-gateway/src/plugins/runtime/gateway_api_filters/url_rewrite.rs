//! URLRewrite filter implementation
//!
//! This filter rewrites upstream hostname and/or path before forwarding.

use async_trait::async_trait;

use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{
    HTTPURLRewriteFilter, PreparedPathModifier, PreparedUrlRewrite, RequestTransformViolation,
};

use super::path_modifier::replace_prefix_match;

pub struct URLRewriteFilter {
    config: PreparedUrlRewrite,
}

impl URLRewriteFilter {
    pub fn try_new(config: &HTTPURLRewriteFilter) -> Result<Self, RequestTransformViolation> {
        Ok(Self {
            config: config.prepare()?,
        })
    }

    fn rewritten_uri(&self, session: &mut dyn PluginSession) -> Option<Result<String, ()>> {
        let path_modifier = self.config.path()?;

        let rewritten_path = match path_modifier {
            PreparedPathModifier::ReplaceFullPath(replacement) => replacement.clone(),
            PreparedPathModifier::ReplacePrefixMatch(replacement) => {
                let original_path = session.get_path();
                let matched_prefix = session
                    .ctx()
                    .route_unit
                    .as_ref()
                    .and_then(|unit| unit.matched_info.m.path.as_ref())
                    .and_then(|path_match| {
                        if path_match.match_type.as_deref() == Some("PathPrefix") {
                            path_match.value.as_deref()
                        } else {
                            None
                        }
                    });
                let Some(matched_prefix) = matched_prefix else {
                    return Some(Err(()));
                };
                replace_prefix_match(original_path, matched_prefix, replacement)
            }
        };

        let query = session.get_query();
        let mut final_uri = rewritten_path;
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            final_uri.reserve(query.len() + 1);
            final_uri.push('?');
            final_uri.push_str(&query);
        }

        Some(Ok(final_uri))
    }
}

#[async_trait]
impl RequestFilter for URLRewriteFilter {
    fn name(&self) -> &str {
        "URLRewrite"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) -> PluginRunningResult {
        let rewritten_uri = match self.rewritten_uri(session) {
            Some(Ok(uri)) => Some(uri),
            Some(Err(())) => {
                plugin_log.push_err("path rewrite prefix context unavailable");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
            None => None,
        };
        if let Some(uri) = rewritten_uri {
            if session.set_upstream_uri(&uri).is_err() {
                plugin_log.push_err("path rewrite failed");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
            plugin_log.push("path rewrite");
        }
        if let Some(hostname) = self.config.hostname() {
            if session.set_upstream_host(hostname).is_err() {
                plugin_log.push_err("host rewrite failed");
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
            plugin_log.push("host rewrite");
        }
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/url_rewrite_test.rs"]
mod tests;
