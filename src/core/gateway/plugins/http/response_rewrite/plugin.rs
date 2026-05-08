//! Response Rewrite plugin implementation
//!
//! Rewrites responses before returning to client.
//! Supports status code and headers modification.

use crate::core::gateway::plugins::runtime::log::la;
use crate::core::gateway::plugins::runtime::{PluginLog, PluginSession, UpstreamResponseFilter};
use crate::types::filters::PluginRunningResult;
use crate::types::resources::edgion_plugins::ResponseRewriteConfig;

/// Response Rewrite plugin
///
/// Rewrites responses before returning to client.
pub struct ResponseRewrite {
    name: String,
    config: ResponseRewriteConfig,
}

impl ResponseRewrite {
    /// Create a new ResponseRewrite plugin from configuration
    pub fn new(config: &ResponseRewriteConfig) -> Self {
        ResponseRewrite {
            name: "ResponseRewrite".to_string(),
            config: config.clone(),
        }
    }

    /// Rewrite response status code
    fn rewrite_status_code(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        if let Some(status_code) = self.config.status_code {
            if let Err(_e) = session.set_response_status(status_code) {
                plugin_log.push_err("status rewrite fail");
            } else {
                plugin_log.push_fmt("status rewrite {}", la![status_code.to_string().as_str()]);
            }
        }
    }

    /// Modify response headers
    ///
    /// Execution order: rename -> add -> set -> remove
    fn modify_headers(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        if let Some(ref headers) = self.config.headers {
            let mut modified = false;

            // Execution order: rename -> add -> set -> remove

            // 1. Rename headers first
            if let Some(ref rename_headers) = headers.rename {
                for entry in rename_headers {
                    // Get the value of the original header
                    if let Some(value) = session.get_response_header(&entry.from) {
                        // Set the new header with the same value
                        if let Err(_e) = session.set_response_header(&entry.to, &value) {
                            plugin_log.push_err("header rename fail");
                        } else {
                            // Remove the old header
                            let _ = session.remove_response_header(&entry.from);
                            modified = true;
                        }
                    }
                }
            }

            // 2. Add headers (append)
            if let Some(ref add_headers) = headers.add {
                for entry in add_headers {
                    if let Err(_e) = session.append_response_header(&entry.name, &entry.value) {
                        plugin_log.push_err("header add fail");
                    } else {
                        modified = true;
                    }
                }
            }

            // 3. Set headers (overwrite)
            if let Some(ref set_headers) = headers.set {
                for entry in set_headers {
                    if let Err(_e) = session.set_response_header(&entry.name, &entry.value) {
                        plugin_log.push_err("header set fail");
                    } else {
                        modified = true;
                    }
                }
            }

            // 4. Remove headers
            if let Some(ref remove_headers) = headers.remove {
                for name in remove_headers {
                    if let Err(_e) = session.remove_response_header(name) {
                        plugin_log.push_err("header remove fail");
                    } else {
                        modified = true;
                    }
                }
            }

            if modified {
                plugin_log.push("header rewrite");
            }
        }
    }
}

impl UpstreamResponseFilter for ResponseRewrite {
    fn name(&self) -> &str {
        &self.name
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        use crate::types::resources::edgion_plugins::PluginValidation;
        // Check if configuration is valid
        if self.config.validation_error().is_some() {
            plugin_log.push_err("config invalid");
            return PluginRunningResult::GoodNext;
        }

        // 1. Status code rewrite
        self.rewrite_status_code(session, plugin_log);

        // 2. Headers modification
        self.modify_headers(session, plugin_log);

        // Continue response chain
        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gateway::plugins::runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::{HeaderRename, ResponseHeaderActions};

    #[test]
    fn test_headers_rename_nonexistent() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: None,
                add: None,
                remove: None,
                rename: Some(vec![HeaderRename {
                    from: "X-NonExistent".to_string(),
                    to: "X-New".to_string(),
                }]),
            }),
        };
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        // Header doesn't exist, return None
        mock_session
            .expect_get_response_header()
            .with(mockall::predicate::eq("X-NonExistent"))
            .returning(|_| None);

        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        // No modification since header doesn't exist
        assert!(!plugin_log.contains("header rewrite"));
    }

    #[test]
    fn test_invalid_status_code() {
        let config = ResponseRewriteConfig {
            status_code: Some(600), // Invalid
            headers: None,
        };
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        // With invalid config, the plugin should skip processing
        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("err: config invalid"));
    }
}
