//! Response Rewrite plugin implementation
//!
//! Rewrites responses before returning to client.
//! Supports status code and headers modification.

use crate::core::plugins::plugin_runtime::{PluginLog, PluginSession, UpstreamResponseFilter};
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
        let mut config = config.clone();
        // Validate configuration
        config.validate();

        ResponseRewrite {
            name: "ResponseRewrite".to_string(),
            config,
        }
    }

    /// Rewrite response status code
    fn rewrite_status_code(&self, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        if let Some(status_code) = self.config.status_code {
            if let Err(e) = session.set_response_status(status_code) {
                plugin_log.push(&format!("Status code rewrite failed: {}; ", e));
            } else {
                plugin_log.push(&format!("Status -> {}; ", status_code));
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
                        if let Err(e) = session.set_response_header(&entry.to, &value) {
                            plugin_log.push(&format!(
                                "Header rename {} -> {} failed: {}; ",
                                entry.from, entry.to, e
                            ));
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
                    if let Err(e) = session.append_response_header(&entry.name, &entry.value) {
                        plugin_log.push(&format!("Header add {} failed: {}; ", entry.name, e));
                    } else {
                        modified = true;
                    }
                }
            }

            // 3. Set headers (overwrite)
            if let Some(ref set_headers) = headers.set {
                for entry in set_headers {
                    if let Err(e) = session.set_response_header(&entry.name, &entry.value) {
                        plugin_log.push(&format!("Header set {} failed: {}; ", entry.name, e));
                    } else {
                        modified = true;
                    }
                }
            }

            // 4. Remove headers
            if let Some(ref remove_headers) = headers.remove {
                for name in remove_headers {
                    if let Err(e) = session.remove_response_header(name) {
                        plugin_log.push(&format!("Header remove {} failed: {}; ", name, e));
                    } else {
                        modified = true;
                    }
                }
            }

            if modified {
                plugin_log.push("Headers modified; ");
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
        // Check if configuration is valid
        if !self.config.is_valid() {
            if let Some(err) = self.config.get_validation_error() {
                plugin_log.push(&format!("Config error: {}; ", err));
            }
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
    use crate::core::plugins::plugin_runtime::traits::session::MockPluginSession;
    use crate::types::resources::edgion_plugins::{
        HeaderRename, ResponseHeaderActions, ResponseHeaderEntry,
    };

    fn create_basic_config() -> ResponseRewriteConfig {
        ResponseRewriteConfig {
            status_code: Some(200),
            headers: None,
            validation_error: None,
        }
    }

    #[test]
    fn test_status_code_rewrite() {
        let config = create_basic_config();
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        mock_session
            .expect_set_response_status()
            .with(mockall::predicate::eq(200u16))
            .returning(|_| Ok(()));

        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Status -> 200"));
    }

    #[test]
    fn test_headers_set() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: Some(vec![
                    ResponseHeaderEntry {
                        name: "X-Custom".to_string(),
                        value: "custom-value".to_string(),
                    },
                    ResponseHeaderEntry {
                        name: "Cache-Control".to_string(),
                        value: "no-cache".to_string(),
                    },
                ]),
                add: None,
                remove: None,
                rename: None,
            }),
            validation_error: None,
        };
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        // Headers should be set in order
        let mut seq = mockall::Sequence::new();
        mock_session
            .expect_set_response_header()
            .with(
                mockall::predicate::eq("X-Custom"),
                mockall::predicate::eq("custom-value"),
            )
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));
        mock_session
            .expect_set_response_header()
            .with(
                mockall::predicate::eq("Cache-Control"),
                mockall::predicate::eq("no-cache"),
            )
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(()));

        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Headers modified"));
    }

    #[test]
    fn test_headers_add() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: None,
                add: Some(vec![ResponseHeaderEntry {
                    name: "X-Added".to_string(),
                    value: "added-value".to_string(),
                }]),
                remove: None,
                rename: None,
            }),
            validation_error: None,
        };
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        mock_session
            .expect_append_response_header()
            .with(
                mockall::predicate::eq("X-Added"),
                mockall::predicate::eq("added-value"),
            )
            .returning(|_, _| Ok(()));

        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Headers modified"));
    }

    #[test]
    fn test_headers_remove() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: None,
                add: None,
                remove: Some(vec!["Server".to_string(), "X-Debug".to_string()]),
                rename: None,
            }),
            validation_error: None,
        };
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        mock_session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("Server"))
            .returning(|_| Ok(()));
        mock_session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("X-Debug"))
            .returning(|_| Ok(()));

        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Headers modified"));
    }

    #[test]
    fn test_headers_rename() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: None,
                add: None,
                remove: None,
                rename: Some(vec![HeaderRename {
                    from: "X-Internal-Id".to_string(),
                    to: "X-Request-Id".to_string(),
                }]),
            }),
            validation_error: None,
        };
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        // First, get the original header value
        mock_session
            .expect_get_response_header()
            .with(mockall::predicate::eq("X-Internal-Id"))
            .returning(|_| Some("internal-123".to_string()));

        // Then set the new header
        mock_session
            .expect_set_response_header()
            .with(
                mockall::predicate::eq("X-Request-Id"),
                mockall::predicate::eq("internal-123"),
            )
            .returning(|_, _| Ok(()));

        // Finally remove the old header
        mock_session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("X-Internal-Id"))
            .returning(|_| Ok(()));

        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Headers modified"));
    }

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
            validation_error: None,
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
        assert!(!plugin_log.contains("Headers modified"));
    }

    #[test]
    fn test_combined_operations() {
        let config = ResponseRewriteConfig {
            status_code: Some(201),
            headers: Some(ResponseHeaderActions {
                set: Some(vec![ResponseHeaderEntry {
                    name: "Content-Type".to_string(),
                    value: "application/json".to_string(),
                }]),
                add: Some(vec![ResponseHeaderEntry {
                    name: "X-Powered-By".to_string(),
                    value: "Edgion".to_string(),
                }]),
                remove: Some(vec!["Server".to_string()]),
                rename: Some(vec![HeaderRename {
                    from: "X-Old".to_string(),
                    to: "X-New".to_string(),
                }]),
            }),
            validation_error: None,
        };
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        // Status code
        mock_session
            .expect_set_response_status()
            .with(mockall::predicate::eq(201u16))
            .returning(|_| Ok(()));

        // Rename (executed first in headers)
        mock_session
            .expect_get_response_header()
            .with(mockall::predicate::eq("X-Old"))
            .returning(|_| Some("old-value".to_string()));
        mock_session
            .expect_set_response_header()
            .with(
                mockall::predicate::eq("X-New"),
                mockall::predicate::eq("old-value"),
            )
            .returning(|_, _| Ok(()));
        mock_session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("X-Old"))
            .returning(|_| Ok(()));

        // Add (executed second)
        mock_session
            .expect_append_response_header()
            .with(
                mockall::predicate::eq("X-Powered-By"),
                mockall::predicate::eq("Edgion"),
            )
            .returning(|_, _| Ok(()));

        // Set (executed third)
        mock_session
            .expect_set_response_header()
            .with(
                mockall::predicate::eq("Content-Type"),
                mockall::predicate::eq("application/json"),
            )
            .returning(|_, _| Ok(()));

        // Remove (executed last)
        mock_session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("Server"))
            .returning(|_| Ok(()));

        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Status -> 201"));
        assert!(plugin_log.contains("Headers modified"));
    }

    #[test]
    fn test_invalid_status_code() {
        let config = ResponseRewriteConfig {
            status_code: Some(600), // Invalid
            headers: None,
            validation_error: None,
        };
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        // With invalid config, the plugin should skip processing
        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert!(plugin_log.contains("Config error"));
    }

    #[test]
    fn test_no_config() {
        let config = ResponseRewriteConfig::default();
        let plugin = ResponseRewrite::new(&config);
        let mut mock_session = MockPluginSession::new();
        let mut plugin_log = PluginLog::new("ResponseRewrite");

        // No expectations - nothing should be called
        let result = plugin.run_upstream_response_filter(&mut mock_session, &mut plugin_log);

        assert_eq!(result, PluginRunningResult::GoodNext);
    }
}
