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
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::{HTTPPathModifier, HTTPPathModifierType};

    #[tokio::test]
    async fn run_request_sets_exact_upstream_path_and_raw_query() {
        let filter = URLRewriteFilter::try_new(&HTTPURLRewriteFilter {
            hostname: None,
            path: Some(HTTPPathModifier {
                modifier_type: HTTPPathModifierType::ReplaceFullPath,
                replace_full_path: Some("/v2/orders".to_string()),
                replace_prefix_match: None,
            }),
        })
        .unwrap();
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

    #[tokio::test]
    async fn upstream_uri_setter_failure_fails_closed() {
        let filter = URLRewriteFilter::try_new(&HTTPURLRewriteFilter {
            hostname: None,
            path: Some(HTTPPathModifier {
                modifier_type: HTTPPathModifierType::ReplaceFullPath,
                replace_full_path: Some("/v2/orders".to_string()),
                replace_prefix_match: None,
            }),
        })
        .unwrap();
        let mut session = MockPluginSession::new();
        session.expect_get_query().return_const(None);
        session
            .expect_set_upstream_uri()
            .times(1)
            .returning(|_| Err(Box::new(std::io::Error::other("setter failed"))));

        let mut log = PluginLog::new("test");
        assert!(matches!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        ));
    }

    #[tokio::test]
    async fn prefix_rewrite_without_matched_prefix_fails_closed() {
        let filter = URLRewriteFilter::try_new(&HTTPURLRewriteFilter {
            hostname: None,
            path: Some(HTTPPathModifier {
                modifier_type: HTTPPathModifierType::ReplacePrefixMatch,
                replace_full_path: None,
                replace_prefix_match: Some("/v2".to_string()),
            }),
        })
        .unwrap();
        let mut session = MockPluginSession::new();
        session.expect_get_path().return_const("/api/orders".to_string());
        session.expect_ctx().return_const(crate::ctx::EdgionHttpContext::new());
        session.expect_set_upstream_uri().times(0);

        let mut log = PluginLog::new("test");
        assert!(matches!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: None
            }
        ));
    }
}
