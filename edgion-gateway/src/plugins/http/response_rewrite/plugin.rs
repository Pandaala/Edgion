//! Response Rewrite plugin implementation
//!
//! Rewrites responses before returning to client.
//! Supports status code and headers modification.

use crate::plugins::runtime::{PluginLog, PluginSession, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{PluginValidation, ResponseHeaderActions, ResponseRewriteConfig};
use edgion_resources::response_header_policy::is_protected_response_header;

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
                    if is_protected_response_header(&entry.from) || is_protected_response_header(&entry.to) {
                        plugin_log.push_err("header rename fail");
                        continue;
                    }
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
#[path = "../../../../tests/unit/plugins/http/response_rewrite/plugin_test.rs"]
mod tests;
