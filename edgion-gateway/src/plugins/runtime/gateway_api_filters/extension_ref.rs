//! ExtensionRef filter implementation
//!
//! This filter handles references to EdgionPlugins resources.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use pingora_http::ResponseHeader;

use crate::plugins::runtime::log::EdgionPluginsLogToken;
use crate::plugins::runtime::log::PluginLog;
use crate::plugins::runtime::traits::{
    PluginSession, RequestFilter, UpstreamResponse, UpstreamResponseBodyFilter, UpstreamResponseFilter,
};
use crate::plugins::runtime::PluginRuntime;
use edgion_resources::filters::{PluginRunningResult, PluginRunningStage};
use edgion_resources::resources::extension_ref::is_supported_edgion_plugins_ref;
use edgion_resources::resources::LocalObjectReference;

/// Error response body written when an ExtensionRef cannot be resolved
/// (cycle, depth exceeded, referenced EdgionPlugins not found).
const EXTENSION_REF_ERROR_BODY: &str = r#"{"message":"plugin reference error"}"#;

/// Write a 500 response with a diagnostic JSON body for an async stage that
/// hit a prepare-execution error. Called from `run_extension_async`'s `Err`
/// arm so clients don't see a naked 502 with no log context.
async fn write_extension_ref_error_response(session: &mut dyn PluginSession) {
    match ResponseHeader::build(500, None) {
        Ok(mut hdr) => {
            let _ = hdr.insert_header("Content-Type", "application/json");
            if session.write_response_header(Box::new(hdr), false).await.is_ok() {
                let _ = session
                    .write_response_body(Some(Bytes::from_static(EXTENSION_REF_ERROR_BODY.as_bytes())), true)
                    .await;
            }
        }
        Err(_) => {
            // ResponseHeader::build(500, ...) cannot fail in practice.
            // If the session is already broken, fall through to ErrTerminateRequest
            // and let the runtime surface the termination.
        }
    }
}

/// Precomputed ExtensionRef target shared by all stage-specific filter clones.
struct ExtensionRefTarget {
    name: String,
    lookup_key: String,
    supported: bool,
}

/// Filter that handles ExtensionRef to EdgionPlugins
#[derive(Clone)]
pub struct ExtensionRefFilter {
    /// Immutable target data shared by the four stage-specific clones.
    target: Arc<ExtensionRefTarget>,
    /// Position in the rule's ExtensionRef list, assigned at route build time.
    /// `Some(idx)` -> look up resolved target via `session.resolved_route_ext_ref(idx)`.
    /// `None` -> backend-level filter, resolved through the request-scoped
    /// pinned cache. Set to `None` initially; rule-scope builder patches to
    /// `Some`.
    index: Option<usize>,
}

impl ExtensionRefFilter {
    pub fn new(namespace: String, ext_ref: LocalObjectReference) -> Self {
        let supported = is_supported_edgion_plugins_ref(&ext_ref);
        let lookup_key = format!("{}/{}", namespace, ext_ref.name);
        Self {
            target: Arc::new(ExtensionRefTarget {
                name: ext_ref.name,
                lookup_key,
                supported,
            }),
            index: None,
        }
    }

    /// Assign the rule-scope index. Called once by the rule-scope builder
    /// (`PluginRuntime::add_from_httproute_filters_inner` with assign_indices=true)
    /// before the four-stage clone fanout; each clone inherits the set index.
    pub(crate) fn set_index(&mut self, index: usize) {
        self.index = Some(index);
    }

    /// Return the precomputed lookup key: namespace/name.
    fn plugin_key(&self) -> &str {
        &self.target.lookup_key
    }

    /// Check if this extension ref points to EdgionPlugins
    fn is_edgion_plugins(&self) -> bool {
        self.target.supported
    }

    /// Resolve the referenced plugin runtime without touching PluginLog or
    /// `pending_edgion_plugins_logs`.
    ///
    /// This is the body-stage path: body filters run per chunk and do not
    /// surface PluginLog entries, so calling `start_edgion_plugins_log` here
    /// would push entries that no stage drains, leaking into
    /// the request's pending EdgionPlugins log queue for the entire request
    /// lifetime (one entry per chunk on long streams). This sibling helper
    /// returns just the runtime, with no log side-effects at all.
    fn resolve_runtime_silent(&self, session: &mut dyn PluginSession) -> Option<Arc<PluginRuntime>> {
        if !self.is_edgion_plugins() {
            return None;
        }
        let runtime = match self.index {
            Some(idx) => session.resolved_route_ext_ref(idx),
            None => session.pinned_backend_ext_ref(self.plugin_key()),
        }?;
        if self.index.is_none() && (runtime.has_waf() || runtime.has_request_id() || runtime.has_upstream_jump_plugin())
        {
            return None;
        }
        Some(runtime)
    }

    /// Helper to prepare plugin execution: validation, lookup, logging setup
    /// Returns the resolved runtime and log token, or an error for an
    /// unsupported target or missing referenced EdgionPlugins resource.
    fn prepare_execution(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> Result<(Arc<PluginRuntime>, EdgionPluginsLogToken), ()> {
        if !self.is_edgion_plugins() {
            // An unsupported ExtensionRef target is a static config error caught
            // at preparse time (`find_invalid_rule_details` /
            // `find_invalid_rule_indices`). This is the runtime backstop — fail
            // closed via `Err(())`, which both callers map to HTTP 500.
            log.push_err("extension ref target not supported");
            return Err(());
        }

        log.set_refer_to(self.target.name.clone());

        // Resolve target: rule-scope reads the pre-resolved Vec via index;
        // backend-scope resolves through the request-pinned cache.
        let plugin_runtime = match self.index {
            Some(idx) => match session.resolved_route_ext_ref(idx) {
                Some(rt) => rt,
                None => {
                    log.push_err("extension ref not found");
                    return Err(());
                }
            },
            None => match session.pinned_backend_ext_ref(self.plugin_key()) {
                Some(rt) => rt,
                None => {
                    log.push_err("extension ref not found");
                    return Err(());
                }
            },
        };

        if self.index.is_none()
            && (plugin_runtime.has_waf()
                || plugin_runtime.has_request_id()
                || plugin_runtime.has_upstream_jump_plugin())
        {
            log.push_err("backend-scoped WAF, RequestId, or upstream jump plugin is not supported");
            return Err(());
        }

        let token = session.start_edgion_plugins_log(self.target.name.clone());
        Ok((plugin_runtime, token))
    }

    /// Execute the referenced EdgionPlugins's plugin runtime
    fn run_extension(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let Ok((plugin_runtime, log_token)) = self.prepare_execution(session, log) else {
            // Sync stage: no `await`, so we cannot write a response body
            // here. Signal the status via `ErrResponse` instead. The
            // response runner (`check_response_stage_terminate`) turns this
            // into a local reply: it builds a fresh `500` and `response_filter`
            // writes it to the client, replacing the upstream response.
            // `body` is `None`, so the client gets a bare `500` (no JSON
            // envelope); the request-stage `run_extension_async` path is the
            // one that writes a JSON body. A bare `ErrTerminateRequest` would
            // trip the runtime's 502 fail-safe, masking this as a plugin-reference
            // error.
            return PluginRunningResult::ErrResponse {
                status: 500,
                body: None,
            };
        };

        match stage {
            PluginRunningStage::UpstreamResponseFilter => {
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    // Install this inner filter's dye plan before invocation,
                    // mirroring `PluginRuntime::run_upstream_response_plugins_sync`
                    // — an ExtensionRef bundle iterates its nested runtime's
                    // filters directly rather than through that loop, so this
                    // call site must do it itself or the bundle's dye rules
                    // never render (response-stage: no strip).
                    session.install_dye_plan(plugin.dye_plan().cloned());
                    let suppress_log = plugin.suppress_automatic_log();
                    let mut inner_log = if suppress_log {
                        PluginLog::disabled()
                    } else {
                        PluginLog::with_alias(plugin.name(), plugin.alias())
                    };
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    if !suppress_log {
                        session.push_to_edgion_plugins_log(&log_token, inner_log);
                    }

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::Request | PluginRunningStage::UpstreamResponse => {
                // Not executed in sync path
                PluginRunningResult::GoodNext
            }
        }
    }

    /// Async version for stages that require it
    async fn run_extension_async(
        &self,
        stage: PluginRunningStage,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        let Ok((plugin_runtime, log_token)) = self.prepare_execution(session, log) else {
            // Response stage: a direct write + ErrTerminateRequest would be
            // forwarded over by the upstream response (the response runner only
            // aborts on an ErrResponse local-reply, not on ErrTerminateRequest).
            // Return ErrResponse so `check_response_stage_terminate` builds the
            // single replacement reply. (Normally unreachable here because
            // `run_upstream_response` pre-resolves; this guards the TOCTOU window
            // where the ref vanishes between the two lookups.)
            if matches!(stage, PluginRunningStage::UpstreamResponse) {
                return PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                };
            }
            // Request stage: without writing a response here, ErrTerminateRequest
            // would surface as a naked 502 (no plugin name in access log, no body)
            // — practically impossible to triage. Write a 500 + JSON body so the
            // terminated request is diagnosable from the client side too.
            write_extension_ref_error_response(session).await;
            return PluginRunningResult::ErrTerminateRequest;
        };

        match stage {
            PluginRunningStage::Request => {
                let outcome_frame = if plugin_runtime.rate_limit_local_outcome_slots() > 0 {
                    session.begin_rate_limit_local_frame(plugin_runtime.rate_limit_local_outcome_slots())
                } else {
                    None
                };
                for plugin in plugin_runtime.request_plugins_iter() {
                    let previous_identity = session.replace_plugin_entry_identity(
                        crate::ctx::CurrentPluginEntryIdentity::new(plugin.name(), plugin.alias()),
                    );
                    // Install this inner filter's dye plan + run the
                    // request-direction anti-spoof strip before invocation,
                    // mirroring `PluginRuntime::run_request_plugins` — an
                    // ExtensionRef bundle iterates its nested runtime's
                    // filters directly rather than through that loop, so this
                    // call site must do it itself or the bundle's dye rules
                    // (and the strip) never run.
                    session.install_dye_plan(plugin.dye_plan().cloned());
                    session.strip_request_dye_once();
                    let suppress_log = plugin.suppress_automatic_log();
                    let mut inner_log = if suppress_log {
                        PluginLog::disabled()
                    } else {
                        PluginLog::with_alias(plugin.name(), plugin.alias())
                    };
                    let result = plugin.run_request(session, &mut inner_log).await;
                    session.restore_plugin_entry_identity(previous_identity);
                    if !suppress_log {
                        session.push_to_edgion_plugins_log(&log_token, inner_log);
                    }

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        if outcome_frame.is_some() {
                            session.end_rate_limit_local_frame(outcome_frame);
                        }
                        return result;
                    }
                }
                if outcome_frame.is_some() {
                    session.end_rate_limit_local_frame(outcome_frame);
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::UpstreamResponse => {
                for plugin in plugin_runtime.upstream_response_async_plugins_iter() {
                    // See the Request arm above: mirrors
                    // `PluginRuntime::run_upstream_response_plugins_async`
                    // (response-stage: no strip).
                    session.install_dye_plan(plugin.dye_plan().cloned());
                    let suppress_log = plugin.suppress_automatic_log();
                    let mut inner_log = if suppress_log {
                        PluginLog::disabled()
                    } else {
                        PluginLog::with_alias(plugin.name(), plugin.alias())
                    };
                    let result = plugin.run_upstream_response(session, &mut inner_log).await;
                    if !suppress_log {
                        session.push_to_edgion_plugins_log(&log_token, inner_log);
                    }

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
            PluginRunningStage::UpstreamResponseFilter => {
                for plugin in plugin_runtime.upstream_response_plugins_iter() {
                    // See the Request arm above: mirrors
                    // `PluginRuntime::run_upstream_response_plugins_sync`
                    // (response-stage: no strip).
                    session.install_dye_plan(plugin.dye_plan().cloned());
                    let suppress_log = plugin.suppress_automatic_log();
                    let mut inner_log = if suppress_log {
                        PluginLog::disabled()
                    } else {
                        PluginLog::with_alias(plugin.name(), plugin.alias())
                    };
                    let result = plugin.run_upstream_response_filter(session, &mut inner_log);
                    if !suppress_log {
                        session.push_to_edgion_plugins_log(&log_token, inner_log);
                    }

                    if !matches!(result, PluginRunningResult::GoodNext | PluginRunningResult::Nothing) {
                        return result;
                    }
                }
                PluginRunningResult::GoodNext
            }
        }
    }

    /// Run extension ref for body filter stage.
    ///
    /// Resolves silently and dispatches to inner body plugins without any
    /// per-chunk log allocation. See `resolve_runtime_silent` for why.
    fn run_extension_body(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<std::time::Duration> {
        let plugin_runtime = self.resolve_runtime_silent(session)?;

        let mut max_delay: Option<std::time::Duration> = None;
        for plugin in plugin_runtime.upstream_response_body_plugins_iter() {
            // See `run_extension_async`'s Request arm: mirrors
            // `PluginRuntime::run_upstream_response_body_plugins` (response-stage:
            // no strip; no PluginLog here either, matching that loop).
            session.install_dye_plan(plugin.dye_plan().cloned());
            if let Some(d) = plugin.run_upstream_response_body_filter(body, end_of_stream, session) {
                max_delay = Some(match max_delay {
                    Some(current) => current.max(d),
                    None => d,
                });
            }
        }

        max_delay
    }
}

#[async_trait]
impl RequestFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        self.run_extension_async(PluginRunningStage::Request, session, log)
            .await
    }
}

impl UpstreamResponseFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    fn run_upstream_response_filter(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
    ) -> PluginRunningResult {
        self.run_extension(PluginRunningStage::UpstreamResponseFilter, session, log)
    }
}

impl UpstreamResponseBodyFilter for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    fn run_upstream_response_body_filter(
        &self,
        body: &Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> Option<std::time::Duration> {
        self.run_extension_body(body, end_of_stream, session)
    }
}

#[async_trait]
impl UpstreamResponse for ExtensionRefFilter {
    fn name(&self) -> &str {
        "ExtensionRef"
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // Resolve silently first. The async UpstreamResponse stage is rare, so
        // most referenced bundles have zero async plugins; returning early here
        // avoids the per-response log-token + empty-bundle-log overhead that
        // every ExtensionRef route would otherwise pay. A dangling/unsupported
        // ref maps to `ErrResponse` (NOT a direct write): in the response stage
        // the runner only honors `ErrResponse` as a local-reply short-circuit,
        // so a direct write would be forwarded over by the upstream response.
        match self.resolve_runtime_silent(session) {
            Some(rt) if rt.upstream_response_async_plugins_count() > 0 => {
                self.run_extension_async(PluginRunningStage::UpstreamResponse, session, log)
                    .await
            }
            // Tag the log entry as a reference so the access log records it
            // consistently with the other stages (the runner uses `refer_to` to
            // identify ExtensionRef entries and skip a misleading time_cost).
            Some(_) => {
                log.set_refer_to(self.target.name.clone());
                PluginRunningResult::GoodNext
            }
            None => {
                log.set_refer_to(self.target.name.clone());
                PluginRunningResult::ErrResponse {
                    status: 500,
                    body: None,
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/gateway_api_filters/extension_ref_test.rs"]
mod tests;
