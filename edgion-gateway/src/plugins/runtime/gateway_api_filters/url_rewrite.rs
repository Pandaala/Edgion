//! URLRewrite filter implementation
//!
//! This filter rewrites upstream hostname and/or path before forwarding.

use async_trait::async_trait;

use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{PluginSession, RequestFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::{HTTPPathModifierType, HTTPURLRewriteFilter};

use super::path_modifier::replace_prefix_match;

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

        let rewritten_path = match path_modifier.modifier_type {
            HTTPPathModifierType::ReplaceFullPath => path_modifier
                .replace_full_path
                .clone()
                .unwrap_or_else(|| session.get_path().to_string()),
            HTTPPathModifierType::ReplacePrefixMatch => {
                let original_path = session.get_path();
                let replacement = path_modifier.replace_prefix_match.as_deref().unwrap_or(original_path);
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
                replace_prefix_match(original_path, matched_prefix.unwrap_or(original_path), replacement)
            }
        };

        let query = session.get_query();
        let mut final_uri = rewritten_path;
        if let Some(query) = query.filter(|query| !query.is_empty()) {
            final_uri.reserve(query.len() + 1);
            final_uri.push('?');
            final_uri.push_str(&query);
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::HTTPPathModifier;

    #[tokio::test]
    async fn run_request_sets_exact_upstream_path_and_raw_query() {
        let filter = URLRewriteFilter::new(HTTPURLRewriteFilter {
            hostname: None,
            path: Some(HTTPPathModifier {
                modifier_type: HTTPPathModifierType::ReplaceFullPath,
                replace_full_path: Some("/v2/orders".to_string()),
                replace_prefix_match: None,
            }),
        });
        let mut session = MockPluginSession::new();
        session
            .expect_get_query()
            .return_const(Some("q=hello%20world&next=%2Forders%3Fstatus%3Dopen".to_string()));
        session.expect_set_upstream_uri().times(1).returning(|uri| {
            assert_eq!(uri, "/v2/orders?q=hello%20world&next=%2Forders%3Fstatus%3Dopen");
            Ok(())
        });

        let mut log = PluginLog::new("test");
        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
    }
}
