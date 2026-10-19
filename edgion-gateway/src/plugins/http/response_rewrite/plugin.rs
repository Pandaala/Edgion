//! Response Rewrite plugin implementation
//!
//! Rewrites responses before returning to client.
//! Supports status code and headers modification.

use crate::plugins::runtime::{PluginLog, PluginSession, PreparedHeader, UpstreamResponseFilter};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{
    HeaderRename, PluginValidation, ResponseHeaderActions, ResponseHeaderEntry, ResponseRewriteConfig,
};
use edgion_resources::response_header_policy::is_protected_response_header;

const PLUGIN_NAME: &str = "ResponseRewrite";

struct PreparedResponseRewrite {
    status_code: Option<u16>,
    status_log: Option<String>,
    headers: Option<PreparedResponseHeaders>,
}

struct PreparedResponseHeaders {
    rename: Option<Vec<PreparedHeaderRename>>,
    add: Vec<PreparedHeader>,
    set: Vec<PreparedHeader>,
    remove: Option<Vec<String>>,
}

struct PreparedHeaderRename {
    entry: HeaderRename,
    protected: bool,
}

impl PreparedHeaderRename {
    fn new(entry: &HeaderRename) -> Self {
        Self {
            entry: entry.clone(),
            protected: is_protected_response_header(&entry.from) || is_protected_response_header(&entry.to),
        }
    }
}

impl PreparedResponseHeaders {
    fn new(headers: &ResponseHeaderActions) -> Result<Self, http::header::InvalidHeaderValue> {
        fn prepare(entries: &[ResponseHeaderEntry]) -> Result<Vec<PreparedHeader>, http::header::InvalidHeaderValue> {
            entries
                .iter()
                .map(|entry| PreparedHeader::new(entry.name.clone(), &entry.value))
                .collect()
        }
        Ok(Self {
            rename: headers
                .rename
                .as_ref()
                .map(|entries| entries.iter().map(PreparedHeaderRename::new).collect()),
            add: prepare(headers.add.as_deref().unwrap_or_default())?,
            set: prepare(headers.set.as_deref().unwrap_or_default())?,
            remove: headers.remove.clone(),
        })
    }
}

impl PreparedResponseRewrite {
    fn new(config: &ResponseRewriteConfig) -> Option<Self> {
        if config.validation_error().is_some() {
            return None;
        }
        Some(Self {
            status_code: config.status_code,
            status_log: config.status_code.map(|status| format!("status rewrite {status}")),
            headers: config
                .headers
                .as_ref()
                .map(PreparedResponseHeaders::new)
                .transpose()
                .ok()?,
        })
    }
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
        let state = PreparedResponseRewrite::new(config)
            .map(ResponseRewriteState::Valid)
            .unwrap_or(ResponseRewriteState::Invalid);
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
                for rename in rename_headers {
                    if rename.protected {
                        plugin_log.push_err("header rename fail");
                        continue;
                    }
                    let entry = &rename.entry;
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

            // 2. Add headers (append), preserving duplicate configured entries.
            for entry in &headers.add {
                if session
                    .append_response_header_value(entry.name(), entry.value().clone())
                    .is_err()
                {
                    plugin_log.push_err("header add fail");
                } else {
                    modified = true;
                }
            }

            // 3. Set headers (overwrite)
            for entry in &headers.set {
                if session
                    .set_response_header_value(entry.name(), entry.value().clone())
                    .is_err()
                {
                    plugin_log.push_err("header set fail");
                } else {
                    modified = true;
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
