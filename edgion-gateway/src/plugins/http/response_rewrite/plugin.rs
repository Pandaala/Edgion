//! Response Rewrite plugin implementation
//!
//! Rewrites responses before returning to client.
//! Supports status code and headers modification.

use crate::plugins::runtime::{PluginLog, PluginSession, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{PluginValidation, ResponseHeaderActions, ResponseRewriteConfig};

const PLUGIN_NAME: &str = "ResponseRewrite";

struct PreparedResponseRewrite {
    status_code: Option<u16>,
    status_log: Option<String>,
    headers: Option<ResponseHeaderActions>,
}

enum ResponseRewriteState {
    Valid(PreparedResponseRewrite),
    Invalid,
}

/// Response Rewrite plugin.
pub struct ResponseRewrite {
    state: ResponseRewriteState,
}

impl ResponseRewrite {
    /// Create a new ResponseRewrite plugin from configuration
    pub fn new(config: &ResponseRewriteConfig) -> Self {
        let state = if config.validation_error().is_some() {
            ResponseRewriteState::Invalid
        } else {
            ResponseRewriteState::Valid(PreparedResponseRewrite {
                status_code: config.status_code,
                status_log: config.status_code.map(|status| format!("status rewrite {status}")),
                headers: config.headers.clone(),
            })
        };
        Self { state }
    }

    /// Rewrite response status code
    fn rewrite_status_code(
        prepared: &PreparedResponseRewrite,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) {
        if let Some(status_code) = prepared.status_code {
            if let Err(_e) = session.set_response_status(status_code) {
                plugin_log.push_err("status rewrite fail");
            } else if let Some(status_log) = prepared.status_log.as_deref() {
                plugin_log.push(status_log);
            }
        }
    }

    /// Modify response headers
    ///
    /// Execution order: rename -> add -> set -> remove
    fn modify_headers(prepared: &PreparedResponseRewrite, session: &mut dyn PluginSession, plugin_log: &mut PluginLog) {
        if let Some(ref headers) = prepared.headers {
            let mut modified = false;

            // 1. Rename headers first
            if let Some(ref rename_headers) = headers.rename {
                for entry in rename_headers {
                    if let Some(value) = session.get_response_header(&entry.from) {
                        if let Err(_e) = session.set_response_header(&entry.to, &value) {
                            plugin_log.push_err("header rename fail");
                        } else {
                            modified = true;
                            if let Err(_e) = session.remove_response_header(&entry.from) {
                                // The destination is already visible. There is no safe
                                // rollback because it may have overwritten an old value.
                                plugin_log.push_err("header rename fail");
                            }
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
        PLUGIN_NAME
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        plugin_log: &mut PluginLog,
    ) -> PluginRunningResult {
        let ResponseRewriteState::Valid(prepared) = &self.state else {
            plugin_log.push_err("config invalid");
            return PluginRunningResult::GoodNext;
        };

        // 1. Status code rewrite
        Self::rewrite_status_code(prepared, session, plugin_log);

        // 2. Headers modification
        Self::modify_headers(prepared, session, plugin_log);

        PluginRunningResult::GoodNext
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::http::plugin_store::compile_plugins;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use edgion_resources::resources::edgion_plugins::{
        EdgionPlugin, EdgionPlugins, HeaderRename, ResponseHeaderEntry, UpstreamResponseFilterEntry,
    };
    use mockall::Sequence;

    fn header(name: &str, value: &str) -> ResponseHeaderEntry {
        ResponseHeaderEntry {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    fn io_error() -> crate::plugins::runtime::traits::session::PluginSessionError {
        std::io::Error::other("injected test failure").into()
    }

    #[test]
    fn name_is_static_across_instances() {
        let first = ResponseRewrite::new(&ResponseRewriteConfig::default());
        let second = ResponseRewrite::new(&ResponseRewriteConfig::default());

        assert_eq!(first.name(), PLUGIN_NAME);
        assert_eq!(second.name(), PLUGIN_NAME);
        assert!(std::ptr::eq(first.name(), second.name()));
    }

    #[test]
    fn prepared_generation_is_independent_from_caller_config() {
        let mut config = ResponseRewriteConfig {
            status_code: Some(201),
            headers: Some(ResponseHeaderActions {
                set: Some(vec![header("X-Prepared", "original")]),
                add: None,
                remove: None,
                rename: None,
            }),
        };
        let plugin = ResponseRewrite::new(&config);

        config.status_code = Some(202);
        let entry = &mut config.headers.as_mut().unwrap().set.as_mut().unwrap()[0];
        entry.name = "X-Mutated".to_string();
        entry.value = "changed".to_string();

        let ResponseRewriteState::Valid(prepared) = &plugin.state else {
            panic!("valid configuration must build a prepared generation");
        };
        assert_eq!(prepared.status_code, Some(201));
        assert_eq!(prepared.status_log.as_deref(), Some("status rewrite 201"));
        let prepared_entry = &prepared.headers.as_ref().unwrap().set.as_ref().unwrap()[0];
        assert_eq!(prepared_entry.name, "X-Prepared");
        assert_eq!(prepared_entry.value, "original");
    }

    #[test]
    fn status_and_header_actions_preserve_order_and_duplicates() {
        let config = ResponseRewriteConfig {
            status_code: Some(201),
            headers: Some(ResponseHeaderActions {
                set: Some(vec![header("X-Set", "set")]),
                add: Some(vec![header("X-Duplicate", "one"), header("X-Duplicate", "two")]),
                remove: Some(vec!["X-Remove".to_string()]),
                rename: Some(vec![HeaderRename {
                    from: "X-Old".to_string(),
                    to: "X-New".to_string(),
                }]),
            }),
        };
        let plugin = ResponseRewrite::new(&config);
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_set_response_status()
            .with(mockall::predicate::eq(201))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        session
            .expect_get_response_header()
            .with(mockall::predicate::eq("X-Old"))
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(Some("old".to_string()));
        session
            .expect_set_response_header()
            .withf(|name, value| name == "X-New" && value == "old")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("X-Old"))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        session
            .expect_append_response_header()
            .withf(|name, value| name == "X-Duplicate" && value == "one")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_append_response_header()
            .withf(|name, value| name == "X-Duplicate" && value == "two")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_set_response_header()
            .withf(|name, value| name == "X-Set" && value == "set")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("X-Remove"))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        let result = plugin.run_upstream_response_filter(&mut session, &mut log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "status rewrite 201\nheader rewrite");
    }

    #[test]
    fn missing_rename_source_is_a_silent_noop() {
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
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(PLUGIN_NAME);

        session
            .expect_get_response_header()
            .with(mockall::predicate::eq("X-NonExistent"))
            .times(1)
            .return_const(None);

        let result = plugin.run_upstream_response_filter(&mut session, &mut log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "");
    }

    #[test]
    fn writer_failures_use_static_logs_and_processing_continues() {
        let config = ResponseRewriteConfig {
            status_code: Some(201),
            headers: Some(ResponseHeaderActions {
                set: Some(vec![header("X-Set", "value")]),
                add: Some(vec![header("X-Add", "value")]),
                remove: Some(vec!["X-Remove".to_string()]),
                rename: Some(vec![HeaderRename {
                    from: "X-Old".to_string(),
                    to: "X-New".to_string(),
                }]),
            }),
        };
        let plugin = ResponseRewrite::new(&config);
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_set_response_status()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Err(io_error()));
        session
            .expect_get_response_header()
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(Some("old".to_string()));
        session
            .expect_set_response_header()
            .withf(|name, _| name == "X-New")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Err(io_error()));
        session
            .expect_append_response_header()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Err(io_error()));
        session
            .expect_set_response_header()
            .withf(|name, _| name == "X-Set")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Err(io_error()));
        session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("X-Remove"))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Err(io_error()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        let result = plugin.run_upstream_response_filter(&mut session, &mut log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(
            log.render_for_test(),
            "err: status rewrite fail\nerr: header rename fail\nerr: header add fail\nerr: header set fail\nerr: header remove fail"
        );
    }

    #[test]
    fn failed_rename_source_removal_is_logged_without_rollback() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: None,
                add: Some(vec![header("X-After", "continued")]),
                remove: None,
                rename: Some(vec![HeaderRename {
                    from: "X-Old".to_string(),
                    to: "X-New".to_string(),
                }]),
            }),
        };
        let plugin = ResponseRewrite::new(&config);
        let mut session = MockPluginSession::new();
        let mut sequence = Sequence::new();
        session
            .expect_get_response_header()
            .times(1)
            .in_sequence(&mut sequence)
            .return_const(Some("old".to_string()));
        session
            .expect_set_response_header()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        session
            .expect_remove_response_header()
            .with(mockall::predicate::eq("X-Old"))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_| Err(io_error()));
        session
            .expect_append_response_header()
            .withf(|name, value| name == "X-After" && value == "continued")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
        let mut log = PluginLog::new(PLUGIN_NAME);

        let result = plugin.run_upstream_response_filter(&mut session, &mut log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "err: header rename fail\nheader rewrite");
    }

    #[test]
    fn direct_invalid_construction_is_a_logged_noop() {
        let config = ResponseRewriteConfig {
            status_code: Some(600),
            headers: None,
        };
        let plugin = ResponseRewrite::new(&config);
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(PLUGIN_NAME);

        let result = plugin.run_upstream_response_filter(&mut session, &mut log);

        assert_eq!(result, PluginRunningResult::GoodNext);
        assert_eq!(log.render_for_test(), "err: config invalid");
    }

    #[tokio::test]
    async fn compile_plugins_matches_schema_validation_and_seals_invalid_config() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: Some(vec![header("Content-Length", "1")]),
                add: None,
                remove: None,
                rename: None,
            }),
        };
        let mut resource = EdgionPlugins::new("invalid-response-rewrite", Default::default());
        resource.metadata.namespace = Some("default".to_string());
        resource.spec.upstream_response_filter_plugins = Some(vec![UpstreamResponseFilterEntry::new(
            EdgionPlugin::ResponseRewrite(config),
        )]);

        let schema_errors = resource.validate_plugins();
        let compiled = compile_plugins(&resource);

        assert_eq!(compiled.preparse_errors, schema_errors);
        assert_eq!(
            compiled.preparse_errors,
            vec!["upstreamResponseFilterPlugins[0] (ResponseRewrite): headers.set must not target content-length or transfer-encoding"]
        );
        assert!(!compiled.is_activatable);
        assert!(!compiled.retain_previous_on_invalid_update);
        assert_eq!(compiled.plugin_runtime.upstream_response_plugins_count(), 0);
        assert_eq!(compiled.plugin_runtime.request_plugins_count(), 1);

        let filter = compiled.plugin_runtime.request_plugins_iter().next().unwrap();
        assert_eq!(filter.name(), "RejectInvalidConfig");
        let mut session = MockPluginSession::new();
        let mut log = PluginLog::new(filter.name());
        let result = filter.run_request(&mut session, &mut log).await;
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 500,
                body: Some(r#"{"message":"plugin configuration error"}"#.to_string()),
            }
        );
        assert_eq!(
            log.render_for_test(),
            "err: invalid plugin config\nerr: ResponseRewrite"
        );
    }
}
